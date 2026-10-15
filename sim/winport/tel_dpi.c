/*
 * tel_dpi.c - telemetry for the UI. The testbench builds a JSON string and hands
 * it over; this writes it atomically to $TEL_DIR/telemetry.json and appends
 * events to $TEL_DIR/events.jsonl. TEL_SLOW_MS (optional) sleeps on every
 * publish so a human can watch a very short simulation in the UI.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#endif

static char dir[512];
static FILE *evf;
static int slow_ms = -1;

static const char *tel_dir(void)
{
    if (!dir[0]) {
        const char *d = getenv("TEL_DIR");

        snprintf(dir, sizeof dir, "%s", d && *d ? d : ".");
    }
    return dir;
}

void tel_publish(const char *json)
{
    char tmp[600], fin[600];
    FILE *f;

    if (slow_ms < 0) {
        const char *s = getenv("TEL_SLOW_MS");

        slow_ms = s ? atoi(s) : 0;
    }
    snprintf(tmp, sizeof tmp, "%s/telemetry.json.tmp", tel_dir());
    snprintf(fin, sizeof fin, "%s/telemetry.json", tel_dir());
    f = fopen(tmp, "wb");
    if (f) {
        fputs(json, f);
        fclose(f);
#ifdef _WIN32
        if (!MoveFileExA(tmp, fin, MOVEFILE_REPLACE_EXISTING))
            remove(tmp);
#else
        rename(tmp, fin);
#endif
    }
    if (slow_ms > 0) {
#ifdef _WIN32
        Sleep((DWORD) slow_ms);
#else
        usleep((useconds_t) slow_ms * 1000);
#endif
    }
}

void tel_event(const char *json)
{
    if (!evf) {
        char p[600];

        snprintf(p, sizeof p, "%s/events.jsonl", tel_dir());
        evf = fopen(p, "wb");
        if (!evf)
            return;
    }
    fputs(json, evf);
    fputc('\n', evf);
    fflush(evf);
}

/*
 * Board inputs from the UI: $TEL_DIR/control.txt holds "<switches> <buttons>" as
 * decimal numbers. Polled at most every 20 ms of real time. Returns 1 when the
 * values were (re)read, 0 when the file is absent or unchanged-throttled.
 */
int tel_ctrl(int *sw, int *btn)
{
    static long long last_ms = -1000;
    static int cur_sw, cur_btn;
    long long now_ms;
    char p[600];
    FILE *f;
    int a, b;

#ifdef _WIN32
    now_ms = (long long) GetTickCount64();
#else
    now_ms = 0;
#endif
    if (now_ms - last_ms >= 20) {
        last_ms = now_ms;
        snprintf(p, sizeof p, "%s/control.txt", tel_dir());
        f = fopen(p, "rb");
        if (f) {
            if (fscanf(f, "%d %d", &a, &b) == 2) {
                cur_sw = a;
                cur_btn = b;
            }
            fclose(f);
        }
    }
    *sw = cur_sw;
    *btn = cur_btn;
    return 1;
}
