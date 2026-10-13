// vmbench: the operations whose cost a hypervisor changes, each as ns per
// operation, so a native run and a guest run can be compared line by line.
// Built statically against musl (build.sh); the guest's PID 1 runs it when
// the cmdline has `npkbench_vm`, a host runs the same file directly.
#define _GNU_SOURCE
#include <linux/futex.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

static int ncpu;
static int self_cpu;
static atomic_int stop;

static uint64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000u + (uint64_t)ts.tv_nsec;
}

static void pin(int cpu) {
    cpu_set_t s;
    CPU_ZERO(&s);
    CPU_SET(cpu, &s);
    sched_setaffinity(0, sizeof s, &s);
}

// nopeekOS prints every vCPU's exits since the last mark on this CPUID
// leaf; anywhere else it is an unknown leaf and does nothing.
static void mark(void) {
    unsigned a = 0x4E504B00, b, c = 0, d;
    __asm__ volatile("cpuid" : "+a"(a), "=b"(b), "+c"(c), "=d"(d));
}

static void report(const char *name, double ns, const char *unit) {
    printf("[vmbench] %-28s %10.0f ns/%s\n", name, ns, unit);
    fflush(stdout);
    mark();
}

// Threads that keep this mm active on the other CPUs, so every unmap or
// protection change must reach them.
static void *spinner(void *arg) {
    pin((int)(intptr_t)arg);
    while (!atomic_load_explicit(&stop, memory_order_relaxed))
        __asm__ volatile("pause");
    return NULL;
}

static void *sleeper(void *arg) {
    pin((int)(intptr_t)arg);
    while (!atomic_load(&stop)) usleep(1000);
    return NULL;
}

static pthread_t helpers[256];

static void start_helpers(void *(*fn)(void *)) {
    atomic_store(&stop, 0);
    for (int c = 0; c < ncpu && c < 256; c++)
        if (c != self_cpu) pthread_create(&helpers[c], NULL, fn, (void *)(intptr_t)c);
    usleep(20000);
}

static void stop_helpers(void) {
    atomic_store(&stop, 1);
    for (int c = 0; c < ncpu && c < 256; c++)
        if (c != self_cpu) pthread_join(helpers[c], NULL);
}

static void bench_syscall(void) {
    const int n = 1000000;
    uint64_t t0 = now_ns();
    for (int i = 0; i < n; i++) syscall(SYS_getppid);
    report("syscall (getppid)", (double)(now_ns() - t0) / n, "call");
}

static void bench_clock(void) {
    const int n = 1000000;
    struct timespec ts;
    uint64_t t0 = now_ns();
    for (int i = 0; i < n; i++) clock_gettime(CLOCK_MONOTONIC, &ts);
    report("clock_gettime", (double)(now_ns() - t0) / n, "call");
}

// CPUID always exits under VMX and SVM: one guest/host round trip.
static void bench_exit(int cpu) {
    const int n = 200000;
    pin(cpu);
    uint64_t t0 = now_ns();
    for (int i = 0; i < n; i++) {
        unsigned a = 0, b, c = 0, d;
        __asm__ volatile("cpuid" : "+a"(a), "=b"(b), "+c"(c), "=d"(d));
    }
    char name[40];
    snprintf(name, sizeof name, "VM exit (cpuid) on cpu%d", cpu);
    report(name, (double)(now_ns() - t0) / n, "exit");
    pin(self_cpu);
}

static void bench_fault(void) {
    const size_t len = 64u << 20;
    char *p = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) return;
    madvise(p, len, MADV_NOHUGEPAGE);
    uint64_t t0 = now_ns();
    for (size_t o = 0; o < len; o += 4096) p[o] = 1;
    report("page fault, 4K first touch", (double)(now_ns() - t0) / (double)(len / 4096), "fault");
    munmap(p, len);
}

static void bench_memcpy(void) {
    const size_t len = 64u << 20;
    char *a = malloc(len), *b = malloc(len);
    if (!a || !b) return;
    memset(a, 1, len);
    memset(b, 2, len);
    uint64_t t0 = now_ns();
    for (int i = 0; i < 4; i++) {
        memcpy(b, a, len);
        // The copy must be observable, or it is optimized away.
        __asm__ volatile("" : : "r"(b) : "memory");
    }
    report("memcpy 64 MB", (double)(now_ns() - t0) / (4.0 * (double)(len >> 20)), "MB");
    free(a);
    free(b);
}

// One unmap of a touched page: the kernel flushes it on every CPU where
// this mm is active.
static void bench_unmap(const char *name) {
    const int n = 20000;
    uint64_t t0 = now_ns();
    for (int i = 0; i < n; i++) {
        char *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        p[0] = 1;
        munmap(p, 4096);
    }
    report(name, (double)(now_ns() - t0) / n, "unmap");
}

// What a JIT does around every code patch.
static void bench_mprotect(const char *name) {
    const int n = 20000;
    char *p = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    p[0] = 1;
    uint64_t t0 = now_ns();
    for (int i = 0; i < n; i++) {
        mprotect(p, 4096, PROT_READ);
        mprotect(p, 4096, PROT_READ | PROT_WRITE);
        p[0] = (char)i;
    }
    report(name, (double)(now_ns() - t0) / n, "pair");
    munmap(p, 4096);
}

// What scudo does when it hands memory back.
static void bench_madvise(const char *name) {
    const int n = 2000;
    const size_t len = 256u << 10;
    char *p = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    uint64_t t0 = now_ns();
    for (int i = 0; i < n; i++) {
        for (size_t o = 0; o < len; o += 4096) p[o] = 1;
        madvise(p, len, MADV_DONTNEED);
    }
    report(name, (double)(now_ns() - t0) / n, "256K");
    munmap(p, len);
}

static atomic_int ping, pong;

static long futex(atomic_int *a, int op, int v) {
    return syscall(SYS_futex, a, op | FUTEX_PRIVATE_FLAG, v, NULL, NULL, 0);
}

static void *futex_peer(void *arg) {
    pin((int)(intptr_t)arg);
    for (;;) {
        while (atomic_load(&ping) == 0) futex(&ping, FUTEX_WAIT, 0);
        if (atomic_load(&stop)) return NULL;
        atomic_store(&ping, 0);
        atomic_store(&pong, 1);
        futex(&pong, FUTEX_WAKE, 1);
    }
}

// Two threads hand a token back and forth, each asleep in the kernel until
// the other wakes it: two cross-CPU wakeups per round trip.
static void bench_futex(int cpu) {
    const int n = 20000;
    pthread_t t;
    atomic_store(&stop, 0);
    atomic_store(&ping, 0);
    atomic_store(&pong, 0);
    pthread_create(&t, NULL, futex_peer, (void *)(intptr_t)cpu);
    pin(0);
    usleep(10000);
    uint64_t t0 = now_ns();
    for (int i = 0; i < n; i++) {
        atomic_store(&pong, 0);
        atomic_store(&ping, 1);
        futex(&ping, FUTEX_WAKE, 1);
        while (atomic_load(&pong) == 0) futex(&pong, FUTEX_WAIT, 0);
    }
    double ns = (double)(now_ns() - t0) / n;
    atomic_store(&stop, 1);
    atomic_store(&ping, 1);
    futex(&ping, FUTEX_WAKE, 1);
    pthread_join(t, NULL);
    char name[40];
    snprintf(name, sizeof name, "futex round trip cpu0<->%d", cpu);
    report(name, ns, "trip");
}

static void bench_sleep(void) {
    const int n = 2000;
    struct timespec req = {0, 100000};
    uint64_t t0 = now_ns();
    for (int i = 0; i < n; i++) nanosleep(&req, NULL);
    report("nanosleep 100 us, overshoot", (double)(now_ns() - t0) / n - 100000.0, "sleep");
}

int main(void) {
    ncpu = (int)sysconf(_SC_NPROCESSORS_ONLN);
    printf("[vmbench] %d CPUs online\n", ncpu);
    // Linux lets a sleep overshoot by 50 us by default; measure the timer.
    prctl(PR_SET_TIMERSLACK, 1UL, 0, 0, 0);
    pin(0);
    mark();
    bench_syscall();
    bench_clock();
    bench_exit(0);
    if (ncpu > 1) bench_exit(ncpu - 1);
    bench_fault();
    bench_memcpy();
    bench_sleep();
    bench_unmap("unmap, alone");
    bench_mprotect("mprotect pair, alone");
    bench_madvise("madvise 256K, alone");
    if (ncpu > 1) {
        start_helpers(spinner);
        mark();
        bench_unmap("unmap, others running");
        bench_mprotect("mprotect pair, others running");
        bench_madvise("madvise 256K, others running");
        stop_helpers();
        start_helpers(sleeper);
        mark();
        bench_unmap("unmap, others asleep");
        bench_madvise("madvise 256K, others asleep");
        stop_helpers();
        bench_futex(1);
        bench_futex(ncpu - 1);
        // The same shootdowns sent from the last CPU instead of cpu0.
        self_cpu = ncpu - 1;
        pin(self_cpu);
        start_helpers(spinner);
        mark();
        bench_unmap("unmap, others running, from last");
        bench_mprotect("mprotect pair, others, from last");
        stop_helpers();
        self_cpu = 0;
        pin(0);
    }
    printf("[vmbench] done\n");
    return 0;
}
