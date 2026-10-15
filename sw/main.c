/*
 * main.c - bare-metal PL check for my_math_ip on Zybo Z7-20 (ps7_cortexa9_0).
 * Writes an operand to 0x43C00000, reads back 0x43C00004 and compares it with
 * operand * 2 (mod 2^32). Prints over the standalone BSP stdout (UART1).
 */
#include <stdint.h>
#include "xil_io.h"
#include "xil_printf.h"

#define MATH_IP_BASE   0x43C00000U
#define REG0_OFFSET    0x00U
#define REG1_OFFSET    0x04U

/* PS SLCR registers. On hardware ps7_init() does this; in QEMU no FSBL runs. */
#define SLCR_UNLOCK        0xF8000008U
#define SLCR_LOCK          0xF8000004U
#define SLCR_FPGA_RST_CTRL 0xF8000240U
#define SLCR_LVL_SHFTR_EN  0xF8000900U
#define SLCR_UNLOCK_KEY    0x0000DF0DU
#define SLCR_LOCK_KEY      0x0000767BU

static void pl_bringup(void)
{
    Xil_Out32(SLCR_UNLOCK, SLCR_UNLOCK_KEY);
    Xil_Out32(SLCR_LVL_SHFTR_EN, 0xFU);   /* PS<->PL level shifters on */
    Xil_Out32(SLCR_FPGA_RST_CTRL, 0x0U);  /* release FCLK_RESET0..3    */
    Xil_Out32(SLCR_LOCK, SLCR_LOCK_KEY);
}

int main(void)
{
    static const uint32_t vectors[] = {
        0x00000000U, 0x00000001U, 0x00000015U, 0x12345678U,
        0x7FFFFFFFU, 0x80000000U, 0xFFFFFFFFU,
    };
    unsigned failures = 0;
    unsigned i;

    xil_printf("\r\nmy_math_ip co-sim test start\r\n");
    pl_bringup();

    for (i = 0; i < sizeof(vectors) / sizeof(vectors[0]); i++) {
        uint32_t test_val = vectors[i];
        uint32_t expected = test_val * 2U;
        uint32_t echo, result;

        Xil_Out32(MATH_IP_BASE + REG0_OFFSET, test_val);
        echo   = Xil_In32(MATH_IP_BASE + REG0_OFFSET);
        result = Xil_In32(MATH_IP_BASE + REG1_OFFSET);

        if (echo == test_val && result == expected) {
            xil_printf("PASS in=0x%08x out=0x%08x\r\n", test_val, result);
        } else {
            xil_printf("FAIL in=0x%08x echo=0x%08x out=0x%08x exp=0x%08x\r\n",
                       test_val, echo, result, expected);
            failures++;
        }
    }

    if (failures == 0)
        xil_printf("RESULT: ALL PASS\r\n");
    else
        xil_printf("RESULT: %u FAILED\r\n", failures);

    xil_printf("COSIM_DONE\r\n");   /* run_cosim.sh stops on this marker */
    for (;;) { }
    return 0;
}
