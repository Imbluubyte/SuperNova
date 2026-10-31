/* SuperNova garbage collector.
 *
 * Design (see docs/MEMORY.md):
 *   - Non-moving, stop-the-world mark & sweep.
 *   - Heap objects are scanned conservatively word by word, EXCEPT objects
 *     allocated "atomic" (strings, Int/Float/Bool list storage), which hold no
 *     pointers and are never scanned. Numeric data therefore costs nothing to mark.
 *   - Roots are the C stack and registers (conservative, interior pointers
 *     allowed) plus registered global variables. Generated code needs no shadow
 *     stack and no write barriers.
 *   - Small objects (<= 32 KiB) live in 256 KiB spans, one size class per span,
 *     with per-object state bytes. Larger objects get their own malloc block.
 *   - Collection is triggered when the bytes allocated since the last collection
 *     exceed max(4 MiB, live bytes after the last collection), so the heap is
 *     at most about twice the live data.
 *
 * Threading: the collector is single-threaded. Only the thread that called
 * sn_gc_init may allocate or touch heap objects.
 *
 * Environment variables (for debugging and tuning):
 *   SN_GC_STATS=1    print collection count, pause times and peak heap at exit
 *   SN_GC_STRESS=N   collect before every Nth allocation (1 = every allocation)
 *
 * Requires sn_fatal() from snrt.h and a POSIX system (Linux, Android/Bionic).
 */
#ifndef SNGC_H
#define SNGC_H

#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <setjmp.h>
#include <time.h>
#include <signal.h>
#include <unistd.h>
#include <pthread.h>
#include <sys/resource.h>

#define SNGC_ALLOC 1u
#define SNGC_MARK 2u
#define SNGC_ATOMIC 4u
#define SNGC_SPAN_BYTES (256u * 1024u)
#define SNGC_MAX_SMALL 32768u
#define SNGC_NCLASS 40
#define SNGC_MIN_THRESHOLD ((size_t)4 << 20)
#define SNGC_MAX_ALLOC ((size_t)1 << 46)

typedef struct SnSpan {
    char *start;
    size_t size;          /* bytes of payload region */
    uint32_t objsize;     /* 0 => this span is a single large object */
    uint32_t nobjs;
    uint32_t bump;        /* slots [0, bump) have been handed out at least once */
    uint8_t cls;
    uint8_t in_avail;
    void *freelist;       /* singly linked through the first word of free slots */
    struct SnSpan *next_avail;
    uint8_t *state;       /* one byte per object: ALLOC | MARK | ATOMIC */
} SnSpan;

typedef struct { uintptr_t start; SnSpan *s; } SnSpanRef;
typedef struct { const void *p; size_t n; } SnGcRoot;
typedef struct { const char *p; size_t n; } SnGcWork;

static const uint32_t sngc_class_size[SNGC_NCLASS] = {
    16, 32, 48, 64, 80, 96, 112, 128,
    160, 192, 224, 256, 320, 384, 448, 512,
    640, 768, 896, 1024, 1280, 1536, 1792, 2048,
    2560, 3072, 3584, 4096, 5120, 6144, 7168, 8192,
    10240, 12288, 14336, 16384, 20480, 24576, 28672, 32768};
static uint8_t sngc_class_of[(SNGC_MAX_SMALL >> 4) + 1]; /* index (size + 15) >> 4 */

static struct {
    SnSpanRef *spans;
    size_t nspans, cspans;
    uintptr_t lo, hi;
    SnSpan *avail[SNGC_NCLASS];
    size_t since;       /* bytes allocated since the last collection */
    size_t threshold;
    size_t live_bytes;  /* bytes in allocated objects (includes uncollected garbage) */
    size_t peak_bytes;
    uintptr_t stack_base;
    size_t stack_limit;
    uintptr_t co_lo;    /* low end of the running coroutine's stack (0 on the main stack) */
    SnGcRoot *roots;
    size_t nroots, croots;
    SnGcWork *work;
    size_t nwork, cwork;
    uint64_t ncollect, total_ns, max_ns;
    long stress, stress_ctr;
    int in_gc, stats, inited;
} sn_gc;

static void sn_gc_collect(void);

static uint64_t sngc_now_ns(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t)t.tv_sec * 1000000000ull + (uint64_t)t.tv_nsec;
}

/* ---------- span table (sorted by address) ---------- */

static inline size_t sngc_upper_bound(uintptr_t a) {
    size_t lo = 0, hi = sn_gc.nspans;
    while (lo < hi) {
        size_t mid = (lo + hi) >> 1;
        if (sn_gc.spans[mid].start <= a) lo = mid + 1; else hi = mid;
    }
    return lo;
}

static inline SnSpan *sngc_find(uintptr_t a) {
    size_t i = sngc_upper_bound(a);
    if (i == 0) return NULL;
    SnSpan *s = sn_gc.spans[i - 1].s;
    return a < sn_gc.spans[i - 1].start + s->size ? s : NULL;
}

static void sngc_add_span(SnSpan *s) {
    size_t i = sngc_upper_bound((uintptr_t)s->start);
    if (sn_gc.nspans == sn_gc.cspans) {
        size_t nc = sn_gc.cspans ? sn_gc.cspans * 2 : 256;
        SnSpanRef *n = (SnSpanRef *)realloc(sn_gc.spans, nc * sizeof(SnSpanRef));
        if (!n) sn_fatal("<runtime>", "out of memory");
        sn_gc.spans = n;
        sn_gc.cspans = nc;
    }
    memmove(&sn_gc.spans[i + 1], &sn_gc.spans[i], (sn_gc.nspans - i) * sizeof(SnSpanRef));
    sn_gc.spans[i].start = (uintptr_t)s->start;
    sn_gc.spans[i].s = s;
    sn_gc.nspans++;
    if (!sn_gc.lo || (uintptr_t)s->start < sn_gc.lo) sn_gc.lo = (uintptr_t)s->start;
    if ((uintptr_t)s->start + s->size > sn_gc.hi) sn_gc.hi = (uintptr_t)s->start + s->size;
}

static void sngc_remove_span(SnSpan *s) {
    size_t i = sngc_upper_bound((uintptr_t)s->start);
    if (i == 0 || sn_gc.spans[i - 1].s != s) return;
    i--;
    memmove(&sn_gc.spans[i], &sn_gc.spans[i + 1], (sn_gc.nspans - i - 1) * sizeof(SnSpanRef));
    sn_gc.nspans--;
}

/* ---------- allocation ---------- */

static SnSpan *sngc_new_span(int cls) {
    uint32_t osz = sngc_class_size[cls];
    uint32_t n = SNGC_SPAN_BYTES / osz;
    SnSpan *s = (SnSpan *)malloc(sizeof(SnSpan) + n);
    char *mem = (char *)malloc(SNGC_SPAN_BYTES);
    if (!s || !mem) {
        free(s);
        free(mem);
        return NULL;
    }
    s->start = mem;
    s->size = SNGC_SPAN_BYTES;
    s->objsize = osz;
    s->nobjs = n;
    s->bump = 0;
    s->cls = (uint8_t)cls;
    s->in_avail = 1;
    s->freelist = NULL;
    s->state = (uint8_t *)(s + 1);
    memset(s->state, 0, n);
    sngc_add_span(s);
    s->next_avail = sn_gc.avail[cls];
    sn_gc.avail[cls] = s;
    return s;
}

static void *sngc_try_alloc(size_t n, int atomic, size_t *charged) {
    if (n <= SNGC_MAX_SMALL) {
        int cls = sngc_class_of[(n + 15) >> 4];
        SnSpan *s = sn_gc.avail[cls];
        while (s && !s->freelist && s->bump == s->nobjs) {
            s->in_avail = 0;
            s = s->next_avail;
            sn_gc.avail[cls] = s;
        }
        if (!s) {
            s = sngc_new_span(cls);
            if (!s) return NULL;
        }
        void *p;
        uint32_t idx;
        if (s->freelist) {
            p = s->freelist;
            s->freelist = *(void **)p;
            idx = (uint32_t)(((char *)p - s->start) / s->objsize);
        } else {
            idx = s->bump++;
            p = s->start + (size_t)idx * s->objsize;
        }
        s->state[idx] = (uint8_t)(SNGC_ALLOC | (atomic ? SNGC_ATOMIC : 0));
        if (!atomic) memset(p, 0, s->objsize);
        *charged = s->objsize;
        return p;
    }
    size_t sz = (n + 15) & ~(size_t)15;
    SnSpan *s = (SnSpan *)malloc(sizeof(SnSpan) + 1);
    char *mem = (char *)(atomic ? malloc(sz) : calloc(1, sz));
    if (!s || !mem) {
        free(s);
        free(mem);
        return NULL;
    }
    s->start = mem;
    s->size = sz;
    s->objsize = 0;
    s->nobjs = 1;
    s->bump = 1;
    s->cls = 0;
    s->in_avail = 0;
    s->freelist = NULL;
    s->next_avail = NULL;
    s->state = (uint8_t *)(s + 1);
    s->state[0] = (uint8_t)(SNGC_ALLOC | (atomic ? SNGC_ATOMIC : 0));
    sngc_add_span(s);
    *charged = sz;
    return mem;
}

/* atomic != 0: the object contains no pointers and is never scanned.
 * Non-atomic objects are returned zeroed. */
static void *sn_gc_alloc(size_t n, int atomic) {
    if (n == 0) n = 1;
    if (n > SNGC_MAX_ALLOC) sn_fatal("<runtime>", "allocation of %zu bytes is too large", n);
    if (sn_gc.since >= sn_gc.threshold) sn_gc_collect();
    if (sn_gc.stress && ++sn_gc.stress_ctr >= sn_gc.stress) {
        sn_gc.stress_ctr = 0;
        sn_gc_collect();
    }
    size_t charged = 0;
    void *p = sngc_try_alloc(n, atomic, &charged);
    if (!p) {
        sn_gc_collect();
        p = sngc_try_alloc(n, atomic, &charged);
        if (!p) sn_fatal("<runtime>", "out of memory (could not allocate %zu bytes)", n);
    }
    sn_gc.since += charged;
    sn_gc.live_bytes += charged;
    return p;
}

/* Explicitly release an object the caller knows nobody else references
 * (used when a list outgrows its storage). Ignores pointers it does not own. */
static void sn_gc_free(void *p) {
    if (!p) return;
    uintptr_t a = (uintptr_t)p;
    SnSpan *s = sngc_find(a);
    if (!s) return;
    if (!s->objsize) {
        if ((char *)p != s->start || !(s->state[0] & SNGC_ALLOC)) return;
        sn_gc.live_bytes -= s->size;
        sngc_remove_span(s);
        free(s->start);
        free(s);
        return;
    }
    size_t off = a - (uintptr_t)s->start;
    if (off % s->objsize) return;
    uint32_t idx = (uint32_t)(off / s->objsize);
    if (idx >= s->bump || !(s->state[idx] & SNGC_ALLOC)) return;
    s->state[idx] = 0;
    *(void **)p = s->freelist;
    s->freelist = p;
    sn_gc.live_bytes -= s->objsize;
    if (!s->in_avail) {
        s->in_avail = 1;
        s->next_avail = sn_gc.avail[s->cls];
        sn_gc.avail[s->cls] = s;
    }
}

/* ---------- marking ---------- */

static void sngc_push_work(const char *p, size_t n) {
    if (sn_gc.nwork == sn_gc.cwork) {
        size_t nc = sn_gc.cwork ? sn_gc.cwork * 2 : 1024;
        SnGcWork *w = (SnGcWork *)realloc(sn_gc.work, nc * sizeof(SnGcWork));
        if (!w) sn_fatal("<runtime>", "out of memory while collecting garbage");
        sn_gc.work = w;
        sn_gc.cwork = nc;
    }
    sn_gc.work[sn_gc.nwork].p = p;
    sn_gc.work[sn_gc.nwork].n = n;
    sn_gc.nwork++;
}

static inline void sngc_mark_word(uintptr_t w) {
    if (w < sn_gc.lo || w >= sn_gc.hi) return;
    SnSpan *s = sngc_find(w);
    if (!s) return;
    uint32_t idx = 0;
    const char *obj = s->start;
    size_t len = s->size;
    if (s->objsize) {
        idx = (uint32_t)((w - (uintptr_t)s->start) / s->objsize);
        if (idx >= s->bump) return;
        obj = s->start + (size_t)idx * s->objsize;
        len = s->objsize;
    }
    uint8_t st = s->state[idx];
    if ((st & (SNGC_ALLOC | SNGC_MARK)) != SNGC_ALLOC) return;
    s->state[idx] = (uint8_t)(st | SNGC_MARK);
    if (!(st & SNGC_ATOMIC)) sngc_push_work(obj, len);
}

static void sngc_scan(const void *lo, const void *hi) {
    const uintptr_t *p = (const uintptr_t *)(((uintptr_t)lo + 7) & ~(uintptr_t)7);
    const uintptr_t *e = (const uintptr_t *)((uintptr_t)hi & ~(uintptr_t)7);
    for (; p < e; p++) sngc_mark_word(*p);
}

static void sngc_drain(void) {
    while (sn_gc.nwork) {
        SnGcWork w = sn_gc.work[--sn_gc.nwork];
        sngc_scan(w.p, w.p + w.n);
    }
}

/* Scans from the address of its own frame up to the stack base. Being the
 * deepest frame in the collection, this covers every caller frame and the
 * register spill area of sn_gc_collect. */
__attribute__((noinline)) static void sngc_scan_stack(void) {
    volatile uintptr_t here = 0;
    uintptr_t lo = (uintptr_t)&here;
    if (sn_gc.stack_base > lo) sngc_scan((const void *)lo, (const void *)sn_gc.stack_base);
    sngc_drain();
}

/* ---------- sweeping ---------- */

static void sngc_sweep(void) {
    SnSpan *tail[SNGC_NCLASS];
    uint8_t kept_empty[SNGC_NCLASS];
    size_t live = 0, w = 0;
    for (int c = 0; c < SNGC_NCLASS; c++) {
        sn_gc.avail[c] = NULL;
        tail[c] = NULL;
        kept_empty[c] = 0;
    }
    for (size_t i = 0; i < sn_gc.nspans; i++) {
        SnSpanRef ref = sn_gc.spans[i];
        SnSpan *s = ref.s;
        if (!s->objsize) {
            if (s->state[0] & SNGC_MARK) {
                s->state[0] &= (uint8_t)~SNGC_MARK;
                live += s->size;
                sn_gc.spans[w++] = ref;
            } else {
                free(s->start);
                free(s);
            }
            continue;
        }
        void *fl = NULL;
        uint32_t used = 0;
        for (int64_t j = (int64_t)s->bump - 1; j >= 0; j--) {
            uint8_t st = s->state[j];
            if (st & SNGC_MARK) {
                s->state[j] = (uint8_t)(st & ~SNGC_MARK);
                used++;
            } else {
                s->state[j] = 0;
                void *o = s->start + (size_t)j * s->objsize;
                *(void **)o = fl;
                fl = o;
            }
        }
        if (used == 0) {
            if (kept_empty[s->cls]) {
                free(s->start);
                free(s);
                continue;
            }
            kept_empty[s->cls] = 1;
            fl = NULL;
            s->bump = 0; /* an empty span restarts with bump allocation */
        }
        s->freelist = fl;
        live += (size_t)used * s->objsize;
        sn_gc.spans[w++] = ref;
        s->next_avail = NULL;
        s->in_avail = 0;
        if (fl || s->bump < s->nobjs) {
            s->in_avail = 1;
            if (tail[s->cls]) tail[s->cls]->next_avail = s; else sn_gc.avail[s->cls] = s;
            tail[s->cls] = s;
        }
    }
    sn_gc.nspans = w;
    sn_gc.live_bytes = live;
}

/* ---------- collection ---------- */

/* Optional hook for extra roots (suspended coroutine stacks); see snasync.h.
 * It calls sngc_scan() on each range and sngc_drain() when done. */
static void (*sn_gc_extra)(void);

__attribute__((noinline)) static void sngc_mark_all(void *regs) {
    (void)regs;
    for (size_t i = 0; i < sn_gc.nroots; i++)
        sngc_scan(sn_gc.roots[i].p, (const char *)sn_gc.roots[i].p + sn_gc.roots[i].n);
    sngc_drain();
    if (sn_gc_extra) { sn_gc_extra(); sngc_drain(); }
    sngc_scan_stack();
}

__attribute__((noinline)) static void sn_gc_collect(void) {
    if (sn_gc.in_gc || !sn_gc.inited) return;
    sn_gc.in_gc = 1;
    uint64_t t0 = sngc_now_ns();
    if (sn_gc.live_bytes > sn_gc.peak_bytes) sn_gc.peak_bytes = sn_gc.live_bytes;
    jmp_buf regs;
    __builtin_unwind_init(); /* spill callee-saved registers into this frame */
    setjmp(regs);            /* and into the jmp_buf */
    sngc_mark_all(&regs);
    sngc_sweep();
    sn_gc.since = 0;
    sn_gc.threshold = sn_gc.live_bytes > SNGC_MIN_THRESHOLD ? sn_gc.live_bytes : SNGC_MIN_THRESHOLD;
    uint64_t dt = sngc_now_ns() - t0;
    sn_gc.ncollect++;
    sn_gc.total_ns += dt;
    if (dt > sn_gc.max_ns) sn_gc.max_ns = dt;
    sn_gc.in_gc = 0;
}

/* Register a global variable (or any static memory) that may hold heap pointers. */
static void sn_gc_root(const void *p, size_t n) {
    if (sn_gc.nroots == sn_gc.croots) {
        size_t nc = sn_gc.croots ? sn_gc.croots * 2 : 64;
        SnGcRoot *r = (SnGcRoot *)realloc(sn_gc.roots, nc * sizeof(SnGcRoot));
        if (!r) sn_fatal("<runtime>", "out of memory");
        sn_gc.roots = r;
        sn_gc.croots = nc;
    }
    sn_gc.roots[sn_gc.nroots].p = p;
    sn_gc.roots[sn_gc.nroots].n = n;
    sn_gc.nroots++;
}

static size_t sn_gc_heap_bytes(void) { return sn_gc.live_bytes; }

/* ---------- start-up, diagnostics and stack overflow detection ---------- */

static void sngc_print_stats(void) {
    size_t peak = sn_gc.peak_bytes > sn_gc.live_bytes ? sn_gc.peak_bytes : sn_gc.live_bytes;
    fprintf(stderr,
            "[gc] collections=%llu total_pause_ms=%.2f max_pause_ms=%.2f peak_heap_mb=%.2f\n",
            (unsigned long long)sn_gc.ncollect, (double)sn_gc.total_ns / 1e6,
            (double)sn_gc.max_ns / 1e6, (double)peak / (1024.0 * 1024.0));
}

static char sngc_altstack[65536];

static void sngc_segv_handler(int sig, siginfo_t *si, void *ctx) {
    (void)sig;
    (void)ctx;
    uintptr_t a = (uintptr_t)si->si_addr;
    const char *msg;
    if ((a < sn_gc.stack_base + 65536 && a + sn_gc.stack_limit + 524288 > sn_gc.stack_base) ||
        (sn_gc.co_lo && a + 65536 > sn_gc.co_lo && a < sn_gc.co_lo + 65536))
        msg = "RuntimeError: stack overflow (infinite or very deep recursion?)\n";
    else
        msg = "FatalError: invalid memory access. This is a bug in SuperNova; please report it.\n";
    fflush(stdout);
    if (write(2, msg, strlen(msg)) < 0) {}
    _exit(1);
}

static void sn_gc_init(void *frame) {
    if (sn_gc.inited) return;
    int c = 0;
    for (size_t i = 1; i <= (SNGC_MAX_SMALL >> 4); i++) {
        while (sngc_class_size[c] < (i << 4)) c++;
        sngc_class_of[i] = (uint8_t)c;
    }
    sn_gc.stack_base = (uintptr_t)frame + 256;
    pthread_attr_t attr;
    if (pthread_getattr_np(pthread_self(), &attr) == 0) {
        void *addr = NULL;
        size_t size = 0;
        if (pthread_attr_getstack(&attr, &addr, &size) == 0 && addr && size)
            sn_gc.stack_base = (uintptr_t)addr + size;
        pthread_attr_destroy(&attr);
    }
    struct rlimit rl;
    sn_gc.stack_limit = (getrlimit(RLIMIT_STACK, &rl) == 0 && rl.rlim_cur != RLIM_INFINITY)
                            ? (size_t)rl.rlim_cur : (size_t)8 << 20;
    sn_gc.threshold = SNGC_MIN_THRESHOLD;
    const char *e = getenv("SN_GC_STRESS");
    if (e && *e) sn_gc.stress = strtol(e, NULL, 10);
    e = getenv("SN_GC_STATS");
    if (e && *e && *e != '0') {
        sn_gc.stats = 1;
        atexit(sngc_print_stats);
    }
    stack_t ss;
    ss.ss_sp = sngc_altstack;
    ss.ss_size = sizeof sngc_altstack;
    ss.ss_flags = 0;
    if (sigaltstack(&ss, NULL) == 0) {
        struct sigaction sa;
        memset(&sa, 0, sizeof sa);
        sa.sa_sigaction = sngc_segv_handler;
        sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
        sigemptyset(&sa.sa_mask);
        sigaction(SIGSEGV, &sa, NULL);
        sigaction(SIGBUS, &sa, NULL);
    }
    sn_gc.inited = 1;
}

#endif /* SNGC_H */
