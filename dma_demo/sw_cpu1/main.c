/*
 * cpu1/main.c - second Cortex-A9 (ps7_cortexa9_1): data producer for the DMA pipeline.
 *
 * CPU0 owns the DMA engine and the UART. CPU1 fills the next source buffer while
 * the PL transforms the previous one (ping-pong, two slots) and reports its state
 * through the shared mailbox (read by CPU0 and by the UI). No printing here: both
 * CPUs share UART1.
 */
#include <stdint.h>
#include "mailbox.h"

static uint32_t lcg(uint32_t *s) { *s = *s * 1664525U + 1013904223U; return *s; }

static inline void mb_dmb(void) { __asm__ __volatile__("dmb" ::: "memory"); }
static inline void mb_sev(void) { __asm__ __volatile__("dsb\n\tsev" ::: "memory"); }
static inline void mb_wfe(void) { __asm__ __volatile__("wfe" ::: "memory"); }

int main(void)
{
    volatile struct mbox *m = (volatile struct mbox *) MBOX_ADDR;
    uint32_t j, i;

    m->cpu1_state = CPU1_BOOT;
    m->cpu1_alive = 1;
    mb_dmb();
    mb_sev();

    /* wait for CPU0 to publish the job description */
    while (m->magic != MBOX_MAGIC)
        mb_wfe();

    for (j = 0; j < m->jobs_total; j++) {
        unsigned slot = j & 1;
        volatile uint32_t *buf = (volatile uint32_t *) (slot ? BUF_SRC_B : BUF_SRC_A);
        uint32_t seed = MBOX_SEED + j;

        m->cpu1_state = CPU1_IDLE;
        while (m->slot_state[slot] != SLOT_FREE)
            mb_wfe();

        m->cpu1_state = CPU1_FILL;
        m->cpu1_job = j;
        for (i = 0; i < m->job_len; i++)
            buf[i] = lcg(&seed);
        m->cpu1_words += m->job_len;
        m->cpu1_alive++;

        mb_dmb();
        m->slot_job[slot] = j;
        m->slot_state[slot] = SLOT_FILLED;
        mb_sev();
    }

    m->cpu1_state = CPU1_DONE;
    mb_dmb();
    mb_sev();
    for (;;)
        mb_wfe();
    return 0;
}
