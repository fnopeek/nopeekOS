// Native counterpart of microvm/linux/init/src/wakebench.rs: two processes
// bounce one byte through a pair of pipes, pinned to CPU 0 and CPU b, and
// print the round trip per pair. Build: cc -O2 -o /tmp/wakebench tools/wakebench.c
#define _GNU_SOURCE
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define ROUNDS 20000
#define WARMUP 200
#define SLOW_NS 100000

static uint64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000u + ts.tv_nsec;
}

static void pin(int cpu) {
    cpu_set_t s;
    CPU_ZERO(&s);
    CPU_SET(cpu, &s);
    sched_setaffinity(0, sizeof s, &s);
}

int main(int argc, char **argv) {
    int last = argc > 1 ? atoi(argv[1]) : (int)sysconf(_SC_NPROCESSORS_ONLN) - 1;
    for (int b = 0; b <= last; b++) {
        int ping[2], pong[2];
        char c = 0;
        if (pipe(ping) || pipe(pong)) return 1;
        pid_t pid = fork();
        if (pid == 0) {
            pin(b);
            for (int i = 0; i < ROUNDS + WARMUP; i++)
                if (read(ping[0], &c, 1) != 1 || write(pong[1], &c, 1) != 1) break;
            _exit(0);
        }
        pin(0);
        uint64_t sum = 0, max = 0, slow = 0;
        for (int i = 0; i < ROUNDS + WARMUP; i++) {
            uint64_t t0 = now_ns();
            if (write(ping[1], &c, 1) != 1 || read(pong[0], &c, 1) != 1) break;
            uint64_t dt = now_ns() - t0;
            if (i >= WARMUP) {
                sum += dt;
                if (dt > max) max = dt;
                if (dt > SLOW_NS) slow++;
            }
        }
        waitpid(pid, NULL, 0);
        close(ping[0]); close(ping[1]); close(pong[0]); close(pong[1]);
        printf("[wakebench] cpu0<->cpu%d: avg %.1f us round trip, max %llu us, over 100 us: %llu/%d\n",
               b, sum / (double)ROUNDS / 1000.0, (unsigned long long)(max / 1000),
               (unsigned long long)slow, ROUNDS);
    }
    return 0;
}
