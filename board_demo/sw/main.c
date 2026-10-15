/*
 * main.c - "virtual Zybo" demo: UART shell + buttons/LEDs + PL image accelerator.
 *
 *   0x43C00000  dma_xform  (word-per-pixel brightness/contrast, IRQ_F2P[0] = GIC 61)
 *   0x43C10000  gpio_ctrl  (LED/SW/BTN, IRQ_F2P[1] = GIC 62)
 *
 * Type `help` in the UI console. Switches, buttons and LEDs are the UI's virtual board.
 */
#include <stdint.h>
#include "xil_io.h"
#include "xil_printf.h"
#include "xil_cache.h"
#include "xil_exception.h"
#include "xscugic.h"
#include "xparameters.h"
#include "xuartps_hw.h"

#define DMA_BASE    0x43C00000U
#define GPIO_BASE   0x43C10000U
#define D_CTRL      0x00U
#define D_STATUS    0x04U
#define D_SRC       0x08U
#define D_DST       0x0CU
#define D_LEN       0x10U
#define D_SCALE     0x14U
#define D_OFFSET    0x18U
#define D_CHECKSUM  0x1CU
#define G_LED       0x00U
#define G_SW        0x04U
#define G_BTN       0x08U
#define G_EVT       0x0CU
#define G_IRQEN     0x10U
#define G_TICKS     0x14U

#define IRQ_DMA     61U
#define IRQ_GPIO    62U

#define IMG_W       64U
#define IMG_H       64U
#define IMG_WORDS   (IMG_W * IMG_H)
#define IMG_SRC     0x01000000U
#define IMG_DST     0x01100000U

#define SLCR_UNLOCK        0xF8000008U
#define SLCR_LOCK          0xF8000004U
#define SLCR_FPGA_RST_CTRL 0xF8000240U
#define SLCR_LVL_SHFTR_EN  0xF8000900U

static XScuGic gic;
static volatile unsigned dma_irqs, btn_pending, btn_total;

static void dma_isr(void *ref)
{
    (void) ref;
    Xil_Out32(DMA_BASE + D_STATUS, 0x2U);        /* clear DONE: deasserts the level IRQ */
    dma_irqs++;
}

static void gpio_isr(void *ref)
{
    uint32_t e = Xil_In32(GPIO_BASE + G_EVT);

    (void) ref;
    Xil_Out32(GPIO_BASE + G_EVT, e);             /* W1C */
    btn_pending |= e;
    btn_total++;
}

static int irq_setup(void)
{
    XScuGic_Config *cfg = XScuGic_LookupConfig(XPAR_SCUGIC_SINGLE_DEVICE_ID);

    if (!cfg || XScuGic_CfgInitialize(&gic, cfg, cfg->CpuBaseAddress) != XST_SUCCESS)
        return -1;
    Xil_ExceptionInit();
    Xil_ExceptionRegisterHandler(XIL_EXCEPTION_ID_INT,
                                 (Xil_ExceptionHandler) XScuGic_InterruptHandler, &gic);
    XScuGic_Connect(&gic, IRQ_DMA, (Xil_InterruptHandler) dma_isr, NULL);
    XScuGic_Connect(&gic, IRQ_GPIO, (Xil_InterruptHandler) gpio_isr, NULL);
    XScuGic_SetPriorityTriggerType(&gic, IRQ_DMA, 0xA0, 0x1);
    XScuGic_SetPriorityTriggerType(&gic, IRQ_GPIO, 0xA0, 0x1);
    XScuGic_Enable(&gic, IRQ_DMA);
    XScuGic_Enable(&gic, IRQ_GPIO);
    Xil_ExceptionEnable();
    return 0;
}

/* ---- image patterns: one pixel (0..255) per 32-bit word ---- */
static void make_image(int pattern)
{
    volatile uint32_t *p = (volatile uint32_t *) IMG_SRC;
    unsigned x, y;

    for (y = 0; y < IMG_H; y++)
        for (x = 0; x < IMG_W; x++) {
            int v;

            if (pattern == 0) {
                v = (int) (x * 4);                                   /* gradient */
            } else if (pattern == 1) {                               /* disc */
                int dx = (int) x - 32, dy = (int) y - 32, d2 = dx * dx + dy * dy;
                v = d2 > 900 ? 20 : 255 - (d2 * 255) / 900;
            } else {
                v = (((x / 8) + (y / 8)) & 1) ? 200 : 40;            /* checker */
            }
            p[y * IMG_W + x] = (uint32_t) v;
        }
    Xil_DCacheFlushRange(IMG_SRC, IMG_WORDS * 4);
}

static void run_filter(int scale, int off, int sat)
{
    uint32_t t0, t1;
    unsigned before = dma_irqs;

    Xil_Out32(DMA_BASE + D_SRC, IMG_SRC);
    Xil_Out32(DMA_BASE + D_DST, IMG_DST);
    Xil_Out32(DMA_BASE + D_LEN, IMG_WORDS);
    Xil_Out32(DMA_BASE + D_SCALE, (uint32_t) scale);
    Xil_Out32(DMA_BASE + D_OFFSET, (uint32_t) off);
    t0 = Xil_In32(GPIO_BASE + G_TICKS);
    Xil_Out32(DMA_BASE + D_CTRL, 0x1U | 0x2U | (sat ? 0x4U : 0U));
    for (;;) {                                   /* sleep until the DMA interrupt */
        __asm__ __volatile__("cpsid i" ::: "memory");
        if (dma_irqs != before)
            break;
        __asm__ __volatile__("wfi" ::: "memory");
        __asm__ __volatile__("cpsie i" ::: "memory");
    }
    __asm__ __volatile__("cpsie i" ::: "memory");
    t1 = Xil_In32(GPIO_BASE + G_TICKS);
    Xil_DCacheInvalidateRange(IMG_DST, IMG_WORDS * 4);
    xil_printf("filtered %u pixels: out = in*%d%s%d%s, %u PL clocks (%u.%u us), checksum 0x%08x\r\n",
               IMG_WORDS, scale, off < 0 ? "" : "+", off, sat ? " (clamped)" : "",
               t1 - t0, (t1 - t0) / 100, ((t1 - t0) % 100) / 10,
               Xil_In32(DMA_BASE + D_CHECKSUM));
}

/* ---- tiny shell ---- */
static const char *skip_ws(const char *s) { while (*s == ' ') s++; return s; }

static const char *parse_int(const char *s, int *out, int *ok)
{
    int neg = 0, any = 0, v = 0;

    s = skip_ws(s);
    if (*s == '-') { neg = 1; s++; }
    while (*s >= '0' && *s <= '9') { v = v * 10 + (*s - '0'); s++; any = 1; }
    *out = neg ? -v : v;
    *ok = any;
    return s;
}

static int starts(const char *s, const char *kw)
{
    while (*kw) if (*s++ != *kw++) return 0;
    return *s == 0 || *s == ' ';
}

static void status(void)
{
    xil_printf("LED=%x SW=%x BTN=%x  button events=%u  DMA irqs=%u  PL ticks=%u\r\n",
               Xil_In32(GPIO_BASE + G_LED), Xil_In32(GPIO_BASE + G_SW),
               Xil_In32(GPIO_BASE + G_BTN), btn_total, dma_irqs,
               Xil_In32(GPIO_BASE + G_TICKS));
}

static int command(const char *l)
{
    int a, b, c, ok;

    l = skip_ws(l);
    if (!*l)
        return 0;
    if (starts(l, "help")) {
        xil_printf("commands:\r\n"
                   "  status            LED/switch/button state and counters\r\n"
                   "  led <0..15>       drive the 4 LEDs\r\n"
                   "  img <0|1|2>       generate a 64x64 test image (gradient, disc, checker) at 0x%08x\r\n"
                   "  run <scale> <off> [sat]  PL filter: out = in*scale+off (sat = clamp 0..255)\r\n"
                   "                    result image is written to 0x%08x\r\n"
                   "  bright <n>        shortcut: run 1 <n> sat\r\n"
                   "  contrast <n>      shortcut: run <n> <-128*(n-1)> sat\r\n"
                   "  end               stop the simulation\r\n"
                   "  buttons toggle LED n and raise an interrupt\r\n", IMG_SRC, IMG_DST);
    } else if (starts(l, "status")) {
        status();
    } else if (starts(l, "led")) {
        parse_int(l + 3, &a, &ok);
        if (!ok) { xil_printf("usage: led <0..15>\r\n"); return 0; }
        Xil_Out32(GPIO_BASE + G_LED, (uint32_t) a & 0xFU);
        xil_printf("LED=%x\r\n", Xil_In32(GPIO_BASE + G_LED));
    } else if (starts(l, "img")) {
        parse_int(l + 3, &a, &ok);
        make_image(ok ? a : 0);
        xil_printf("image %d written (%ux%u, one pixel per word) at 0x%08x\r\n", ok ? a : 0,
                   IMG_W, IMG_H, IMG_SRC);
    } else if (starts(l, "run")) {
        const char *p = parse_int(l + 3, &a, &ok);
        int ok2;
        p = parse_int(p, &b, &ok2);
        if (!ok || !ok2) { xil_printf("usage: run <scale> <offset> [sat]\r\n"); return 0; }
        p = skip_ws(p);
        parse_int(p, &c, &ok);
        run_filter(a, b, ok ? c : 0);
    } else if (starts(l, "bright")) {
        parse_int(l + 6, &a, &ok);
        run_filter(1, ok ? a : 40, 1);
    } else if (starts(l, "contrast")) {
        parse_int(l + 8, &a, &ok);
        if (!ok) a = 2;
        run_filter(a, -128 * (a - 1), 1);
    } else if (starts(l, "end")) {
        xil_printf("bye\r\nCOSIM_DONE\r\n");
        Xil_Out32(DMA_BASE + 0x3CU, 0xC051D0DEU);
        return 1;
    } else {
        xil_printf("unknown command (try `help`)\r\n");
    }
    return 0;
}

int main(void)
{
    char line[64];
    unsigned n = 0;

    Xil_Out32(SLCR_UNLOCK, 0xDF0DU);
    Xil_Out32(SLCR_LVL_SHFTR_EN, 0xFU);
    Xil_Out32(SLCR_FPGA_RST_CTRL, 0x0U);
    Xil_Out32(SLCR_LOCK, 0x767BU);

    xil_printf("\r\nvirtual Zybo Z7-20 ready (type `help`)\r\n");
    if (irq_setup()) {
        xil_printf("interrupt setup failed\r\n");
        return 1;
    }
    Xil_Out32(GPIO_BASE + G_IRQEN, 0xFU);        /* any button raises IRQ_F2P[1] */
    make_image(1);
    xil_printf("> ");

    for (;;) {
        if (btn_pending) {
            uint32_t e;

            __asm__ __volatile__("cpsid i" ::: "memory");
            e = btn_pending;
            btn_pending = 0;
            __asm__ __volatile__("cpsie i" ::: "memory");
            xil_printf("\r\n[button event 0x%x, total %u]\r\n> ", e, btn_total);
            Xil_Out32(GPIO_BASE + G_LED, Xil_In32(GPIO_BASE + G_LED) ^ (e & 0xFU));
        }
        if (XUartPs_IsReceiveData(STDIN_BASEADDRESS)) {
            char ch = (char) XUartPs_ReadReg(STDIN_BASEADDRESS, XUARTPS_FIFO_OFFSET);

            if (ch == '\r' || ch == '\n') {
                line[n] = 0;
                n = 0;
                xil_printf("\r\n");
                if (command(line))
                    break;
                xil_printf("> ");
            } else if (ch == 0x7f || ch == 8) {
                if (n) { n--; xil_printf("\b \b"); }
            } else if (n < sizeof line - 1) {
                line[n++] = ch;
                outbyte(ch);
            }
        }
    }
    for (;;) { }
    return 0;
}
