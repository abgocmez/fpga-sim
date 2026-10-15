/*
 * rp_dpi.c - minimal QEMU remote-port client for XSim over DPI-C.
 *
 * Plays the PL side of Xilinx's remote-port protocol (the same wire protocol
 * libsystemctlm-soc speaks) without SystemC/TLM, so it builds with xsc's DPI
 * flow (xsc + xelab -sv_lib) on Windows and Linux.
 *
 * PS -> PL accesses (QEMU is the master, Verilog is an AXI4-Lite slave):
 *   rp_open()            connect, exchange hello
 *   rp_next(...)  -> 1   a 32-bit beat is ready (is_write/addr/wdata/wstrb)
 *                    2   nothing pending right now; advance sim time and retry
 *                    0   peer closed the connection; finish the simulation
 *   rp_done(rdata, ok)   complete that beat (the response is sent after the
 *                        last beat of the bus access)
 *
 * PL -> PS accesses (Verilog is an AXI master into QEMU's memory, e.g. S_AXI_HP0):
 *   rp_mem_put(i, w)             stage write word i
 *   rp_mem_start(...)    -> slot start a read/write of `len` bytes at `addr`
 *   rp_mem_poll(slot)    -> 0    pending, 1 done ok, 2 done with error, -1 EOF
 *   rp_mem_get(slot, i)          read word i of a finished read
 *   rp_mem_free(slot)
 *
 * PL -> PS interrupts: rp_irq(now, line, val)  (remote-port wires_in, device 9)
 *
 * All entry points are non-reentrant and single threaded; any of them may pump
 * incoming packets, so concurrent Verilog processes can share the connection.
 * Interrupt (from QEMU), cfg and ATS packets are consumed and ignored; wire
 * updates are acknowledged (CAP_WIRE_POSTED_UPDATES) so PL->PS interrupts are applied
 * before PL time moves on.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <winsock2.h>
#include "wsdyn.h"
#else
#include <sys/select.h>
#include <sys/time.h>
#endif

#include "remote-port-proto.h"
#include "remote-port-sk.h"
#include "safeio.h"

#define RX_MAX      (64 * 1024)
#define DATA_MAX    4096
#define REQ_Q       8
#define NSLOT       4
#define RP_DEV_WIRES_IN 9      /* PL -> PS interrupt lines (zynq-pl-remoteport) */

static int fd = -1;
static struct rp_peer_state peer;
static uint32_t tx_id;
static unsigned char rx_buf[RX_MAX];
static unsigned char tx_buf[RX_MAX];

/* ---- PS -> PL bus accesses, queued until the testbench serves them ---- */
struct ps_req {
    struct rp_pkt pkt;                 /* copy of the request, for the response */
    int      is_write;
    uint64_t addr;
    uint32_t len;
    uint32_t off;
    int      started;                  /* first beat handed out */
    long long t_first;                 /* PL time when service started (ns) */
    unsigned char wdata[DATA_MAX];
    unsigned char rdata[DATA_MAX];
};
static struct ps_req reqq[REQ_Q];
static int rq_head, rq_count;

/* ---- time lock: QEMU sync packets are answered only once PL time caught up ---- */
static int lockstep;
static int sync_pending;
static uint32_t sync_id, sync_dev;
static uint64_t sync_ts;

/* non-posted wire (interrupt) update in flight: its response id */
static int irq_wait;
static uint32_t irq_wait_id;

/* ---- PL -> PS requests ---- */
struct mem_slot {
    int      used;
    int      done;
    int      ok;
    int      is_write;
    uint32_t id;
    uint32_t len;
    unsigned char data[DATA_MAX];
};
static struct mem_slot slots[NSLOT];
static unsigned char stage[DATA_MAX];

static struct rp_pkt *rx_pkt(void) { return (struct rp_pkt *) rx_buf; }

static int rp_wr(const void *p, size_t n)
{
    return rp_safe_write(fd, p, n) == (ssize_t) n ? 0 : -1;
}

static int rp_rd(void *p, size_t n)
{
    return rp_safe_read(fd, p, n) == (ssize_t) n ? 0 : -1;
}

static int send_hello(void)
{
    uint32_t caps[] = { CAP_BUSACCESS_EXT_BASE, CAP_WIRE_POSTED_UPDATES };
    uint32_t caps_out[2];
    struct rp_pkt_hello pkt;
    size_t len;

    memset(&pkt, 0, sizeof pkt);
    len = rp_encode_hello_caps(tx_id++, 0, &pkt, RP_VERSION_MAJOR,
                               RP_VERSION_MINOR, caps, caps_out, 2);
    if (rp_wr(&pkt, len) || rp_wr(caps_out, sizeof caps_out))
        return -1;
    return 0;
}

int rp_open(const char *descr)
{
    if (!descr || !*descr)
        descr = getenv("RP_SOCKET");
    if (!descr || !*descr) {
        fprintf(stderr, "rp_open: RP_SOCKET is not set\n");
        return -1;
    }
    fd = sk_open(descr);
    if (fd < 0) {
        fprintf(stderr, "rp_open: cannot connect to %s\n", descr);
        return -1;
    }
    memset(&peer, 0, sizeof peer);
    if (send_hello()) {
        fprintf(stderr, "rp_open: hello failed\n");
        return -1;
    }
    return 0;
}

static int data_ready(int timeout_ms)
{
    fd_set rd;
    struct timeval tv;

    FD_ZERO(&rd);
    FD_SET((unsigned) fd, &rd);
    if (timeout_ms < 0)                  /* block until a packet (or EOF) arrives */
        return select(fd + 1, &rd, NULL, NULL, NULL) > 0;
    tv.tv_sec = timeout_ms / 1000;
    tv.tv_usec = (timeout_ms % 1000) * 1000;
    return select(fd + 1, &rd, NULL, NULL, &tv) > 0;
}

static void send_busaccess_resp(struct ps_req *r, int resp)
{
    struct rp_encode_busaccess_in in;
    struct rp_pkt *tx = (struct rp_pkt *) tx_buf;
    unsigned char *d;
    size_t plen;

    if (r->pkt.hdr.flags & RP_PKT_FLAGS_posted)
        return;                          /* posted write: no response */

    rp_encode_busaccess_in_rsp_init(&in, &r->pkt);
    d = rp_busaccess_tx_dataptr(&peer, &tx->busaccess_ext_base);
    if (!r->is_write)
        memcpy(d, r->rdata, r->len);
    in.clk = r->pkt.busaccess.timestamp;
    in.attr |= (uint64_t) resp << RP_BUS_RESP_SHIFT;
    plen = rp_encode_busaccess(&peer, &tx->busaccess_ext_base, &in);
    rp_wr(tx, plen);
}

static void handle_nonbus(struct rp_pkt *p)
{
    switch (p->hdr.cmd) {
    case RP_CMD_hello:
        if (p->hello.caps.len)
            rp_process_caps(&peer, (char *) p + p->hello.caps.offset,
                            p->hello.caps.len);
        break;
    case RP_CMD_sync: {
        if (lockstep) {                  /* answered by rp_sync_ack() */
            sync_pending = 1;
            sync_id = p->hdr.id;
            sync_dev = p->hdr.dev;
            sync_ts = p->sync.timestamp;
        } else {
            struct rp_pkt_sync *r = (struct rp_pkt_sync *) tx_buf;
            size_t n = rp_encode_sync_resp(p->hdr.id, p->hdr.dev, r,
                                           (int64_t) p->sync.timestamp);
            rp_wr(r, n);
        }
        break;
    }
    case RP_CMD_interrupt:
        if (!(p->hdr.flags & RP_PKT_FLAGS_posted)) {   /* PS->PL wire (e.g. reset): ack */
            struct rp_pkt_interrupt *r = (struct rp_pkt_interrupt *) tx_buf;
            size_t n = rp_encode_interrupt_f(p->hdr.id, p->hdr.dev, r,
                                             (int64_t) p->interrupt.timestamp,
                                             p->interrupt.line, p->interrupt.vector,
                                             p->interrupt.val,
                                             p->hdr.flags | RP_PKT_FLAGS_response);
            rp_wr(r, n);
        }
        break;
    default:                             /* cfg, ats...: ignore */
        break;
    }
}

/* Reads and dispatches one packet. Returns -1 on EOF/error, else 0. */
static int pump_one(void)
{
    struct rp_pkt *p = rx_pkt();
    uint32_t dlen;
    unsigned char *data;

    if (rp_rd(&p->hdr, sizeof p->hdr))
        return -1;
    rp_decode_hdr(p);
    if (p->hdr.len > RX_MAX - sizeof p->hdr)
        return -1;
    if (p->hdr.len && rp_rd((char *) &p->hdr + sizeof p->hdr, p->hdr.len))
        return -1;
    dlen = rp_decode_payload(p);
    data = rx_buf + sizeof p->hdr + dlen;

    if (p->hdr.flags & RP_PKT_FLAGS_response) {
        int i;

        if (irq_wait && p->hdr.id == irq_wait_id) {
            irq_wait = 0;
            return 0;
        }

        for (i = 0; i < NSLOT; i++) {
            struct mem_slot *s = &slots[i];

            if (!s->used || s->done || s->id != p->hdr.id)
                continue;
            s->ok = rp_get_busaccess_response(p) == RP_RESP_OK;
            if (!s->is_write) {
                unsigned char *rd = rp_busaccess_rx_dataptr(&peer, &p->busaccess_ext_base);
                memcpy(s->data, rd, s->len);
            }
            s->done = 1;
            break;
        }
        return 0;
    }

    if (p->hdr.cmd == RP_CMD_write || p->hdr.cmd == RP_CMD_read) {
        struct ps_req *r;
        uint32_t len = p->busaccess.len;

        if (rq_count == REQ_Q || len > DATA_MAX) {
            fprintf(stderr, "rp_dpi: dropping PS access (queue full or len %u)\n", len);
            return 0;
        }
        r = &reqq[(rq_head + rq_count) % REQ_Q];
        rq_count++;
        memcpy(&r->pkt, p, sizeof r->pkt);
        r->is_write = p->hdr.cmd == RP_CMD_write;
        r->addr = p->busaccess.addr;
        r->len = len;
        r->off = 0;
        memset(r->rdata, 0, len);
        if (r->is_write)
            memcpy(r->wdata, data, len);
        return 0;
    }
    handle_nonbus(p);
    return 0;
}

/* Pumps at most one packet. 1 = processed one, 0 = none ready, -1 = EOF. */
int rp_poll(int timeout_ms)
{
    if (fd < 0)
        return -1;
    if (!data_ready(timeout_ms))
        return 0;
    return pump_one() ? -1 : 1;
}

/* ------------------------------------------------------------------ PS -> PL */

int rp_next(long long now_ns, int idle_ms, int *addr, int *wdata, int *wstrb,
            int *is_write)
{
    for (;;) {
        if (rq_count) {
            struct ps_req *r = &reqq[rq_head];
            uint64_t a = r->addr + r->off;

            if (!r->started) {
                r->started = 1;
                r->t_first = now_ns;
            }
            unsigned lane = (unsigned) (a & 3);
            unsigned n = 4 - lane;
            uint32_t w = 0;
            unsigned i;

            if (n > r->len - r->off)
                n = r->len - r->off;
            if (r->is_write)
                for (i = 0; i < n; i++)
                    w |= (uint32_t) r->wdata[r->off + i] << (8 * (lane + i));
            *addr = (int) (a & ~3ULL);
            *wdata = (int) w;
            *wstrb = (int) (((1u << n) - 1u) << lane);
            *is_write = r->is_write;
            return 1;
        }
        if (sync_pending)
            return 3;                    /* advance PL time to rp_sync_target() */
        switch (rp_poll(idle_ms)) {
        case 0:  return 2;
        case 1:  break;
        default: return 0;
        }
    }
}

void rp_done(int rdata, int ok)
{
    struct ps_req *r;
    uint64_t a;
    unsigned lane, n, i;

    if (!rq_count)
        return;
    r = &reqq[rq_head];
    a = r->addr + r->off;
    lane = (unsigned) (a & 3);
    n = 4 - lane;
    if (n > r->len - r->off)
        n = r->len - r->off;
    if (!r->is_write)
        for (i = 0; i < n; i++)
            r->rdata[r->off + i] = (unsigned char) (((uint32_t) rdata) >> (8 * (lane + i)));
    r->off += n;

    if (r->off >= r->len) {
        send_busaccess_resp(r, ok ? RP_RESP_OK : RP_RESP_BUS_GENERIC_ERROR);
        rq_head = (rq_head + 1) % REQ_Q;
        rq_count--;
    }
}

/* ------------------------------------------------------------- time lock */

void rp_lockstep(int on) { lockstep = on; }

/* PL time (ns) QEMU wants us to reach before it continues, or -1 */
long long rp_sync_target(void) { return sync_pending ? (long long) sync_ts : -1; }

void rp_sync_ack(long long now_ns)
{
    struct rp_pkt_sync *r = (struct rp_pkt_sync *) tx_buf;
    size_t n;

    if (!sync_pending)
        return;
    n = rp_encode_sync_resp(sync_id, sync_dev, r, (int64_t) now_ns);
    rp_wr(r, n);
    sync_pending = 0;
}

/* QEMU time stamp (ns) of the PS access at the head of the queue, or -1 */
long long rp_req_ts(void)
{
    return rq_count ? (long long) reqq[rq_head].pkt.busaccess.timestamp : -1;
}

/* like rp_done, but reports the PL-side access latency back to QEMU */
void rp_done_t(int rdata, int ok, long long now_ns)
{
    if (rq_count) {
        struct ps_req *r = &reqq[rq_head];
        long long lat = now_ns - r->t_first;

        if (lat < 0)
            lat = 0;
        r->pkt.busaccess.timestamp += (uint64_t) lat;   /* response clk = ts + latency */
    }
    rp_done(rdata, ok);
}

/* ------------------------------------------------------------------ PL -> PS */

void rp_mem_put(int idx, int word)
{
    if (idx >= 0 && (idx + 1) * 4 <= DATA_MAX) {
        stage[idx * 4 + 0] = (unsigned char) (word);
        stage[idx * 4 + 1] = (unsigned char) (word >> 8);
        stage[idx * 4 + 2] = (unsigned char) (word >> 16);
        stage[idx * 4 + 3] = (unsigned char) (word >> 24);
    }
}

int rp_mem_start(long long now_ns, int dev, int is_write, int addr, int len)
{
    struct rp_encode_busaccess_in in;
    struct rp_pkt *tx = (struct rp_pkt *) tx_buf;
    struct mem_slot *s = NULL;
    size_t plen;
    int i;

    if (len <= 0 || len > DATA_MAX)
        return -1;
    for (i = 0; i < NSLOT; i++)
        if (!slots[i].used) { s = &slots[i]; break; }
    if (!s)
        return -1;

    memset(&in, 0, sizeof in);
    in.cmd = is_write ? RP_CMD_write : RP_CMD_read;
    in.id = tx_id++;
    in.dev = (uint32_t) dev;
    in.clk = now_ns;
    in.addr = (uint64_t) (uint32_t) addr;
    in.size = (uint32_t) len;
    in.width = 0;
    in.stream_width = (uint32_t) len;

    plen = rp_encode_busaccess(&peer, &tx->busaccess_ext_base, &in);
    if (rp_wr(tx, plen))
        return -1;
    if (is_write && rp_wr(stage, (size_t) len))
        return -1;

    s->used = 1;
    s->done = 0;
    s->ok = 0;
    s->is_write = is_write;
    s->id = in.id;
    s->len = (uint32_t) len;
    return i;
}

int rp_mem_poll(int slot)
{
    struct mem_slot *s;

    if (slot < 0 || slot >= NSLOT || !slots[slot].used)
        return -1;
    s = &slots[slot];
    if (!s->done) {
        int r = rp_poll(0);

        if (r < 0)
            return -1;
    }
    if (!s->done)
        return 0;
    return s->ok ? 1 : 2;
}

/* Blocks (real time, no PL time advance) until the response arrives. Keeps the
 * simulation deterministic: the modelled latency is added by the testbench. */
int rp_mem_wait(int slot)
{
    struct mem_slot *s;

    if (slot < 0 || slot >= NSLOT || !slots[slot].used)
        return -1;
    s = &slots[slot];
    while (!s->done)
        if (rp_poll(-1) < 0)
            return -1;
    return s->ok ? 1 : 2;
}

int rp_mem_get(int slot, int idx)
{
    const unsigned char *d;

    if (slot < 0 || slot >= NSLOT || idx < 0 || (unsigned) (idx + 1) * 4 > slots[slot].len)
        return 0;
    d = slots[slot].data + idx * 4;
    return (int) ((uint32_t) d[0] | ((uint32_t) d[1] << 8) |
                  ((uint32_t) d[2] << 16) | ((uint32_t) d[3] << 24));
}

void rp_mem_free(int slot)
{
    if (slot >= 0 && slot < NSLOT)
        slots[slot].used = 0;
}

void rp_irq(long long now_ns, int line, int val)
{
    struct rp_pkt_interrupt p;
    size_t n;
    uint32_t id = tx_id++;
    int nonposted = peer.caps.wire_posted_updates;

    n = rp_encode_interrupt_f(id, RP_DEV_WIRES_IN, &p, now_ns,
                              (uint32_t) line, 0, (uint8_t) val,
                              nonposted ? 0 : RP_PKT_FLAGS_posted);
    rp_wr(&p, n);
    if (nonposted) {
        /* wait until QEMU has applied the wire: keeps delivery deterministic */
        irq_wait = 1;
        irq_wait_id = id;
        while (irq_wait)
            if (rp_poll(-1) < 0)
                break;
        irq_wait = 0;
    }
}

void rp_close(void)
{
    if (fd >= 0) {
#ifdef _WIN32
        closesocket((SOCKET) fd);
#endif
        fd = -1;
    }
}
