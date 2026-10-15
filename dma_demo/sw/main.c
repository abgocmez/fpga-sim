/*
 * main.c - bare-metal test of dma_xform (Zybo Z7-20, ps7_cortexa9_0).
 *   AXI4-Lite control at 0x43C00000, DMA through S_AXI_HP0, completion IRQ on
 *   IRQ_F2P[0] = GIC SPI 29 (interrupt ID 61), level high.
 * Cases: polled, interrupt-driven, 4 KB-crossing buffers, and error handling.
 */
#include <stdint.h>
#include "xil_io.h"
#include "xil_printf.h"
#include "xil_cache.h"
#include "xil_exception.h"
#include "xscugic.h"
#include "xparameters.h"
#include "mailbox.h"

#define DMA_BASE    0x43C00000U
#define R_CTRL      0x00U
#define R_STATUS    0x04U
#define R_SRC       0x08U
#define R_DST       0x0CU
#define R_LEN       0x10U
#define R_SCALE     0x14U
#define R_OFFSET    0x18U
#define R_CHECKSUM  0x1CU
#define R_WORDS     0x20U

#define CTRL_START  0x1U
#define CTRL_IRQEN  0x2U
#define ST_BUSY     0x1U
#define ST_DONE     0x2U
#define ST_ERR      0x4U

#define IRQ_F2P0_ID 61U            /* SPI 29 */

#define BUF_SRC     0x01000000U
#define BUF_DST     0x01100000U

/* PS SLCR: no FSBL runs in QEMU, so enable the PL level shifters / resets. */
#define SLCR_UNLOCK        0xF8000008U
#define SLCR_LOCK          0xF8000004U
#define SLCR_FPGA_RST_CTRL 0xF8000240U
#define SLCR_LVL_SHFTR_EN  0xF8000900U

static XScuGic gic;
static volatile unsigned irq_count;

static void dma_isr(void *ref)
{
    (void) ref;
    /* level interrupt: clear DONE at the source, which deasserts the line */
    Xil_Out32(DMA_BASE + R_STATUS, ST_DONE);
    irq_count++;
}

static int gic_setup(void)
{
    XScuGic_Config *cfg = XScuGic_LookupConfig(XPAR_SCUGIC_SINGLE_DEVICE_ID);

    if (!cfg || XScuGic_CfgInitialize(&gic, cfg, cfg->CpuBaseAddress) != XST_SUCCESS)
        return -1;
    Xil_ExceptionInit();
    Xil_ExceptionRegisterHandler(XIL_EXCEPTION_ID_INT,
                                 (Xil_ExceptionHandler) XScuGic_InterruptHandler, &gic);
    if (XScuGic_Connect(&gic, IRQ_F2P0_ID, (Xil_InterruptHandler) dma_isr, NULL) != XST_SUCCESS)
        return -1;
    XScuGic_SetPriorityTriggerType(&gic, IRQ_F2P0_ID, 0xA0, 0x1);   /* level high */
    XScuGic_Enable(&gic, IRQ_F2P0_ID);
    Xil_ExceptionEnable();
    return 0;
}

/* Sleep in WFI until the PL interrupt count moves past `before` (IRQs masked around
 * the check so a wake-up cannot be lost). */
static void wait_irq(unsigned before)
{
    for (;;) {
        __asm__ __volatile__("cpsid i" ::: "memory");
        if (irq_count != before)
            break;
        __asm__ __volatile__("wfi" ::: "memory");
        __asm__ __volatile__("cpsie i" ::: "memory");
    }
    __asm__ __volatile__("cpsie i" ::: "memory");
}

/* (dmb/dsb/wfe/sev are macros in the Xilinx headers, hence the mb_ prefix) */
static inline void mb_dmb(void) { __asm__ __volatile__("dmb" ::: "memory"); }
static inline void mb_sev(void) { __asm__ __volatile__("dsb\n\tsev" ::: "memory"); }
static inline void mb_wfe(void) { __asm__ __volatile__("wfe" ::: "memory"); }

static uint32_t lcg(uint32_t *s) { *s = *s * 1664525U + 1013904223U; return *s; }

/* returns number of mismatches (>0), or 0 on success */
static unsigned run_case(const char *name, uint32_t src, uint32_t dst, uint32_t len,
                         uint32_t scale, uint32_t off, int use_irq)
{
    volatile uint32_t *s = (volatile uint32_t *) src;
    volatile uint32_t *d = (volatile uint32_t *) dst;
    uint32_t seed = src ^ len, sum = 0, i, st, spin;
    unsigned bad = 0, irq_before = irq_count;

    for (i = 0; i < len; i++)
        s[i] = lcg(&seed);
    for (i = 0; i < len + 2; i++)
        d[i] = 0xDEADBEEFU;
    Xil_DCacheFlushRange(src, len * 4);
    Xil_DCacheFlushRange(dst, (len + 2) * 4);

    Xil_Out32(DMA_BASE + R_SRC, src);
    Xil_Out32(DMA_BASE + R_DST, dst);
    Xil_Out32(DMA_BASE + R_LEN, len);
    Xil_Out32(DMA_BASE + R_SCALE, scale);
    Xil_Out32(DMA_BASE + R_OFFSET, off);
    Xil_Out32(DMA_BASE + R_CTRL, CTRL_START | (use_irq ? CTRL_IRQEN : 0));

    if (use_irq) {
        /* QEMU runs far ahead of XSim, so a counted spin would time out: sleep in
         * WFI until the PL interrupt arrives (IRQs masked around the check so a
         * wake-up cannot be lost). */
        wait_irq(irq_before);
    } else {
        for (spin = 0; spin < 5000000U; spin++) {
            st = Xil_In32(DMA_BASE + R_STATUS);
            if (st & ST_DONE)
                break;
        }
        Xil_Out32(DMA_BASE + R_STATUS, ST_DONE | ST_ERR);
    }

    Xil_DCacheInvalidateRange(dst, (len + 2) * 4);
    seed = src ^ len;
    for (i = 0; i < len; i++) {
        uint32_t exp = lcg(&seed) * scale + off;

        sum += exp;
        if (d[i] != exp && bad++ < 4)
            xil_printf("  %s: dst[%u]=0x%08x exp 0x%08x\r\n", name, i, d[i], exp);
    }
    if (d[len] != 0xDEADBEEFU) {
        xil_printf("  %s: wrote past the end\r\n", name);
        bad++;
    }
    st = Xil_In32(DMA_BASE + R_CHECKSUM);
    if (st != sum) {
        xil_printf("  %s: checksum 0x%08x exp 0x%08x\r\n", name, st, sum);
        bad++;
    }
    st = Xil_In32(DMA_BASE + R_WORDS);
    if (st != len) {
        xil_printf("  %s: words %u exp %u\r\n", name, st, len);
        bad++;
    }
    xil_printf("%s %s (len=%u, %s)\r\n", bad ? "FAIL" : "PASS", name, len,
               use_irq ? "irq" : "poll");
    return bad;
}

/* Two-CPU pipeline: CPU1 produces source buffers (ping-pong), CPU0 drives the DMA
 * and verifies, so CPU1's fill of job j+1 overlaps the PL work on job j. */
static unsigned run_pipeline(uint32_t jobs, uint32_t len)
{
    struct mbox *m = (struct mbox *) MBOX_ADDR;
    unsigned bad = 0;
    uint32_t j, i;

    m->jobs_total = jobs;
    m->job_len = len;
    m->slot_state[0] = m->slot_state[1] = SLOT_FREE;
    m->cpu0_jobs_done = 0;
    m->cpu0_errors = 0;
    mb_dmb();
    m->magic = MBOX_MAGIC;
    mb_sev();

    for (j = 0; j < jobs; j++) {
        unsigned slot = j & 1;
        uint32_t src = slot ? BUF_SRC_B : BUF_SRC_A;
        uint32_t dst = slot ? BUF_DST_B : BUF_DST_A;
        uint32_t scale = j + 2, off = j * 3, seed = MBOX_SEED + j, sum = 0;
        volatile uint32_t *d = (volatile uint32_t *) dst;
        unsigned before = irq_count, mism = 0;

        m->cpu0_job = j;
        while (m->slot_state[slot] != SLOT_FILLED)
            mb_wfe();
        m->slot_state[slot] = SLOT_INFLIGHT;
        for (i = 0; i < len + 2; i++)
            d[i] = 0xDEADBEEFU;

        Xil_Out32(DMA_BASE + R_SRC, src);
        Xil_Out32(DMA_BASE + R_DST, dst);
        Xil_Out32(DMA_BASE + R_LEN, len);
        Xil_Out32(DMA_BASE + R_SCALE, scale);
        Xil_Out32(DMA_BASE + R_OFFSET, off);
        Xil_Out32(DMA_BASE + R_CTRL, CTRL_START | CTRL_IRQEN);
        wait_irq(before);

        for (i = 0; i < len; i++) {
            uint32_t exp = lcg(&seed) * scale + off;

            sum += exp;
            if (d[i] != exp)
                mism++;
        }
        if (Xil_In32(DMA_BASE + R_CHECKSUM) != sum)
            mism++;
        bad += mism;
        m->cpu0_errors += mism;
        m->cpu0_jobs_done++;
        mb_dmb();
        m->slot_state[slot] = SLOT_FREE;
        mb_sev();
    }
    xil_printf("%s pipeline (%u jobs x %u words, CPU1 produced %u words)\r\n",
               bad ? "FAIL" : "PASS", jobs, len, m->cpu1_words);
    if (m->cpu1_words != jobs * len)
        bad++;
    return bad;
}

int main(void)
{
    unsigned failures = 0;
    uint32_t st;

    xil_printf("\r\ndma_xform co-sim test start\r\n");

    Xil_Out32(SLCR_UNLOCK, 0xDF0DU);
    Xil_Out32(SLCR_LVL_SHFTR_EN, 0xFU);
    Xil_Out32(SLCR_FPGA_RST_CTRL, 0x0U);
    Xil_Out32(SLCR_LOCK, 0x767BU);

    if (gic_setup()) {
        xil_printf("FAIL gic setup\r\n");
        goto done;
    }

    failures += run_case("polled-64",    BUF_SRC,          BUF_DST,          64,   3, 7,  0);
    failures += run_case("irq-1000",     BUF_SRC,          BUF_DST,          1000, 5, 1,  1);
    failures += run_case("cross4k-200",  BUF_SRC + 0xF80,  BUF_DST + 0xF00,  200,  2, 0,  1);
    failures += run_case("single-word",  BUF_SRC + 0x40,   BUF_DST + 0x40,   1,    9, 9,  0);

    failures += run_pipeline(6, 512);

    /* error handling: unaligned source must be rejected without any DMA */
    Xil_Out32(DMA_BASE + R_SRC, BUF_SRC + 2);
    Xil_Out32(DMA_BASE + R_DST, BUF_DST);
    Xil_Out32(DMA_BASE + R_LEN, 8);
    Xil_Out32(DMA_BASE + R_CTRL, CTRL_START);
    st = Xil_In32(DMA_BASE + R_STATUS);
    if ((st & (ST_ERR | ST_DONE)) == (ST_ERR | ST_DONE)) {
        xil_printf("PASS unaligned-src rejected\r\n");
    } else {
        xil_printf("FAIL unaligned-src status=0x%x\r\n", st);
        failures++;
    }
    Xil_Out32(DMA_BASE + R_STATUS, ST_DONE | ST_ERR);

done:
    if (failures == 0)
        xil_printf("RESULT: ALL PASS\r\n");
    else
        xil_printf("RESULT: %u FAILED\r\n", failures);
    xil_printf("COSIM_DONE\r\n");
    Xil_Out32(DMA_BASE + 0x3CU, 0xC051D0DEU);   /* tells the testbench to end the simulation */
    for (;;) { }
    return 0;
}
