/* mailbox.h - shared state between CPU0, CPU1 (and the UI, which peeks at it
 * through the QEMU monitor). Lives in DDR, outside both program images. */
#ifndef MAILBOX_H
#define MAILBOX_H

#include <stdint.h>

#define MBOX_ADDR     0x01F00000U
#define MBOX_MAGIC    0xC0DEC0DEU
#define MBOX_SEED     0x1234U

#define BUF_SRC_A     0x01000000U
#define BUF_SRC_B     0x01010000U
#define BUF_DST_A     0x01100000U
#define BUF_DST_B     0x01110000U

enum { SLOT_FREE = 0, SLOT_FILLED = 1, SLOT_INFLIGHT = 2 };
enum { CPU1_BOOT = 0, CPU1_IDLE = 1, CPU1_FILL = 2, CPU1_DONE = 3 };

struct mbox {
    volatile uint32_t magic;           /* MBOX_MAGIC once CPU0 filled the fields below */
    volatile uint32_t jobs_total;
    volatile uint32_t job_len;         /* words per job */
    volatile uint32_t cpu1_state;      /* CPU1_* */
    volatile uint32_t cpu1_alive;      /* CPU1 heartbeat / progress counter */
    volatile uint32_t cpu1_job;        /* job CPU1 is working on */
    volatile uint32_t cpu1_words;      /* words produced so far */
    volatile uint32_t slot_state[2];   /* SLOT_* */
    volatile uint32_t slot_job[2];
    volatile uint32_t cpu0_job;        /* job CPU0 is processing */
    volatile uint32_t cpu0_jobs_done;
    volatile uint32_t cpu0_errors;
};

#endif
