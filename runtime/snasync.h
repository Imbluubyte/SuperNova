/* SuperNova async runtime: cooperative tasks on separate stacks, timers and
 * background file I/O. Included by generated code only when a program uses
 * async/await.
 *
 * Model: every `async fn` call creates a Task that runs on its own stack
 * (stackful coroutine; the context switch is a few instructions of assembly,
 * because ucontext is missing on Android's libc). Tasks only switch at
 * `await`, so there are no data races on program data. The scheduler runs on
 * the main stack: whenever the program awaits at top level, and before exit,
 * it runs ready tasks, fires timers and collects finished file operations.
 * File operations run on short-lived worker threads that never touch the
 * garbage-collected heap, so `await readFile(...)` does not block other tasks. */
#ifndef SNASYNC_H
#define SNASYNC_H

#include <pthread.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#if !defined(__x86_64__) && !defined(__aarch64__)
#error "async/await currently supports x86-64 and ARM64 only"
#endif

/* void sn_co_switch(void **save_sp, void *new_sp): saves callee-saved
 * registers on the current stack, stores sp, then resumes the context whose
 * saved sp is new_sp. */
void sn_co_switch(void **save_sp, void *new_sp);
#if defined(__x86_64__)
__asm__(
    ".text\n"
    ".globl sn_co_switch\n"
    ".type sn_co_switch,@function\n"
    "sn_co_switch:\n"
    "    pushq %rbp\n    pushq %rbx\n    pushq %r12\n    pushq %r13\n    pushq %r14\n    pushq %r15\n"
    "    movq %rsp, (%rdi)\n"
    "    movq %rsi, %rsp\n"
    "    popq %r15\n    popq %r14\n    popq %r13\n    popq %r12\n    popq %rbx\n    popq %rbp\n"
    "    ret\n"
    ".size sn_co_switch,.-sn_co_switch\n");
#define SN_CO_FRAME_WORDS 8 /* r15 r14 r13 r12 rbx rbp, entry, fake return address */
#else
__asm__(
    ".text\n"
    ".globl sn_co_switch\n"
    ".type sn_co_switch,%function\n"
    "sn_co_switch:\n"
    "    sub sp, sp, #176\n"
    "    stp x19, x20, [sp, #0]\n    stp x21, x22, [sp, #16]\n    stp x23, x24, [sp, #32]\n"
    "    stp x25, x26, [sp, #48]\n    stp x27, x28, [sp, #64]\n    stp x29, x30, [sp, #80]\n"
    "    stp d8, d9, [sp, #96]\n    stp d10, d11, [sp, #112]\n    stp d12, d13, [sp, #128]\n    stp d14, d15, [sp, #144]\n"
    "    mov x9, sp\n"
    "    str x9, [x0]\n"
    "    mov sp, x1\n"
    "    ldp x19, x20, [sp, #0]\n    ldp x21, x22, [sp, #16]\n    ldp x23, x24, [sp, #32]\n"
    "    ldp x25, x26, [sp, #48]\n    ldp x27, x28, [sp, #64]\n    ldp x29, x30, [sp, #80]\n"
    "    ldp d8, d9, [sp, #96]\n    ldp d10, d11, [sp, #112]\n    ldp d12, d13, [sp, #128]\n    ldp d14, d15, [sp, #144]\n"
    "    add sp, sp, #176\n"
    "    ret\n"
    ".size sn_co_switch,.-sn_co_switch\n");
#endif

enum { SN_TS_NEW, SN_TS_READY, SN_TS_RUNNING, SN_TS_BLOCKED, SN_TS_DONE };

typedef struct SnTask {
    int state;
    int failed, observed, kind;     /* kind: 0 coroutine, 1 timer, 2 file operation */
    void (*entry)(struct SnTask *);
    void *sp;                       /* saved stack pointer while suspended */
    char *stack;                    /* mmap'ed region (guard page first) */
    size_t stack_size;
    SnTry *try_top;                 /* this task's handler stack while suspended */
    struct SnTask *next_ready, *waiters, *wait_next;
    struct SnTask *all_next, *all_prev;   /* live coroutine tasks (GC roots) */
    struct SnTask *aux_next;        /* timer / pending file operation lists */
    struct SnTask *unobs_next;
    int64_t deadline_ns;
    SnStr *exc;
    const char *exc_loc;
    int exc_kind;
    void *io;                       /* SnIo for file operations */
} SnTask;

static SnTask *sn_ready_head, *sn_ready_tail;
static SnTask *sn_all_head;
static SnTask *sn_timers;           /* sorted by deadline */
static SnTask *sn_io_tasks;         /* file operations in flight (keeps them alive) */
static SnTask *sn_unobserved;       /* failed tasks nobody awaited */
static SnTask *sn_cur;              /* running coroutine, or NULL on the main stack */
static void *sn_main_sp;
static uintptr_t sn_main_base;
static size_t sn_task_stack_bytes = 256 * 1024;
static void *sn_stack_pool;         /* freed stacks, linked through their first word */
static int64_t sn_io_pending;

static int64_t sn_now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

/* ---------- stacks ---------- */

static char *sn_stack_alloc(void) {
    if (sn_stack_pool) {
        char *s = (char *)sn_stack_pool;
        sn_stack_pool = *(void **)(s + sysconf(_SC_PAGESIZE));
        return s;
    }
    size_t page = (size_t)sysconf(_SC_PAGESIZE);
    size_t total = sn_task_stack_bytes + page;
    char *s = (char *)mmap(NULL, total, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
    if (s == (char *)MAP_FAILED) sn_fatal("<runtime>", "out of memory (could not allocate a task stack)");
    mprotect(s, page, PROT_NONE); /* guard page: overflow faults instead of corrupting memory */
    return s;
}

static void sn_stack_release(char *s) {
    *(void **)(s + sysconf(_SC_PAGESIZE)) = sn_stack_pool;
    sn_stack_pool = s;
}

/* ---------- scheduler ---------- */

static void sn_ready_push(SnTask *t) {
    t->state = SN_TS_READY;
    t->next_ready = NULL;
    if (sn_ready_tail) sn_ready_tail->next_ready = t; else sn_ready_head = t;
    sn_ready_tail = t;
}

static SnTask *sn_ready_pop(void) {
    SnTask *t = sn_ready_head;
    if (!t) return NULL;
    sn_ready_head = t->next_ready;
    if (!sn_ready_head) sn_ready_tail = NULL;
    t->next_ready = NULL;
    return t;
}

static void sn_all_link(SnTask *t) {
    t->all_prev = NULL;
    t->all_next = sn_all_head;
    if (sn_all_head) sn_all_head->all_prev = t;
    sn_all_head = t;
}

static void sn_all_unlink(SnTask *t) {
    if (t->all_prev) t->all_prev->all_next = t->all_next; else if (sn_all_head == t) sn_all_head = t->all_next;
    if (t->all_next) t->all_next->all_prev = t->all_prev;
    t->all_next = t->all_prev = NULL;
}

static SnTask *sn_task_new(size_t size, void (*entry)(SnTask *)) {
    SnTask *t = (SnTask *)sn_gc_alloc(size, 0);
    t->entry = entry;
    t->kind = 0;
    t->state = SN_TS_NEW;
    return t;
}

static void sn_task_spawn(SnTask *t) {
    sn_all_link(t);
    sn_ready_push(t);
}

/* Mark a task finished and wake everything waiting for it. */
static void sn_task_complete(SnTask *t) {
    t->state = SN_TS_DONE;
    SnTask *w = t->waiters;
    t->waiters = NULL;
    if (t->failed && !w && !t->observed) { t->unobs_next = sn_unobserved; sn_unobserved = t; }
    while (w) {
        SnTask *next = w->wait_next;
        w->wait_next = NULL;
        sn_ready_push(w);
        w = next;
    }
}

static void sn_co_entry(void) {
    SnTask *volatile t = sn_cur;
    SnTry base;
    base.prev = NULL;
    sn_try_top = &base;
    if (setjmp(base.jb) == 0) {
        t->entry(t);
        sn_try_top = NULL;
    } else {
        t->failed = 1;
        t->exc = sn_exc;
        t->exc_loc = sn_exc_loc;
        t->exc_kind = sn_exc_kind;
    }
    sn_task_complete(t);
    sn_co_switch(&t->sp, sn_main_sp);
    abort(); /* a finished coroutine is never resumed */
}

/* Run a task until it blocks or finishes. Called on the main stack only. */
static void sn_resume(SnTask *t) {
    if (!t->stack) {
        t->stack = sn_stack_alloc();
        size_t page = (size_t)sysconf(_SC_PAGESIZE);
        t->stack_size = sn_task_stack_bytes + page;
        uintptr_t top = ((uintptr_t)t->stack + t->stack_size) & ~(uintptr_t)15;
#if defined(__x86_64__)
        uintptr_t *sp = (uintptr_t *)top - SN_CO_FRAME_WORDS;
        for (int i = 0; i < SN_CO_FRAME_WORDS; i++) sp[i] = 0;
        sp[6] = (uintptr_t)sn_co_entry; /* popped by `ret` */
        t->sp = sp;                     /* sp[7] = 0 is the fake return address */
#else
        uintptr_t *sp = (uintptr_t *)(top - 176);
        for (int i = 0; i < 22; i++) sp[i] = 0;
        sp[11] = (uintptr_t)sn_co_entry; /* x30 */
        t->sp = sp;
#endif
        t->try_top = NULL;
    }
    SnTry *saved_try = sn_try_top;
    uintptr_t saved_base = sn_gc.stack_base;
    t->state = SN_TS_RUNNING;
    sn_cur = t;
    sn_try_top = t->try_top;
    sn_gc.stack_base = (uintptr_t)t->stack + t->stack_size;
    sn_gc.co_lo = (uintptr_t)t->stack;
    sn_co_switch(&sn_main_sp, t->sp);
    t->try_top = sn_try_top;
    sn_try_top = saved_try;
    sn_gc.stack_base = saved_base;
    sn_gc.co_lo = 0;
    sn_cur = NULL;
    if (t->state == SN_TS_DONE) {
        sn_all_unlink(t);
        sn_stack_release(t->stack);
        t->stack = NULL;
    }
}

/* Mark the running task blocked and return to the scheduler. */
static void sn_block_current(void) {
    SnTask *me = sn_cur;
    me->state = SN_TS_BLOCKED;
    sn_co_switch(&me->sp, sn_main_sp);
}

/* ---------- timers and background file operations ---------- */

typedef struct SnIo {
    SnTask *task;
    int op;               /* 0 read, 1 write */
    char *path, *data;
    size_t len;
    char *result;
    size_t rlen;
    int err;
    struct SnIo *next;
} SnIo;

static pthread_mutex_t sn_io_mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t sn_io_cv = PTHREAD_COND_INITIALIZER;
static SnIo *sn_io_done;

static void *sn_io_worker(void *arg) {
    SnIo *io = (SnIo *)arg;
    if (io->op == 0) {
        FILE *f = fopen(io->path, "rb");
        if (!f) io->err = errno ? errno : EIO;
        else {
            size_t cap = 4096, n = 0;
            char *buf = (char *)malloc(cap);
            for (;;) {
                if (n == cap) { cap *= 2; buf = (char *)realloc(buf, cap); }
                size_t r = fread(buf + n, 1, cap - n, f);
                n += r;
                if (r == 0) break;
            }
            if (ferror(f)) io->err = EIO;
            fclose(f);
            io->result = buf;
            io->rlen = n;
        }
    } else {
        FILE *f = fopen(io->path, "wb");
        if (!f) io->err = errno ? errno : EIO;
        else {
            if (io->len && fwrite(io->data, 1, io->len, f) != io->len) io->err = EIO;
            if (fclose(f) != 0 && !io->err) io->err = EIO;
        }
    }
    pthread_mutex_lock(&sn_io_mu);
    io->next = sn_io_done;
    sn_io_done = io;
    pthread_cond_signal(&sn_io_cv);
    pthread_mutex_unlock(&sn_io_mu);
    return NULL;
}

static SnTask *sn_io_start(int op, SnStr *path, SnStr *data, size_t task_size) {
    SnTask *t = (SnTask *)sn_gc_alloc(task_size, 0);
    SnIo *io = (SnIo *)calloc(1, sizeof(SnIo));
    io->task = t;
    io->op = op;
    io->path = (char *)malloc((size_t)path->len + 1);
    memcpy(io->path, path->data, (size_t)path->len + 1);
    if (data) {
        io->len = (size_t)data->len;
        io->data = (char *)malloc(io->len + 1);
        memcpy(io->data, data->data, io->len + 1);
    }
    t->kind = 2;
    t->state = SN_TS_BLOCKED;
    t->io = io;
    t->aux_next = sn_io_tasks;
    sn_io_tasks = t;
    sn_io_pending++;
    pthread_t th;
    pthread_attr_t at;
    pthread_attr_init(&at);
    pthread_attr_setdetachstate(&at, PTHREAD_CREATE_DETACHED);
    pthread_attr_setstacksize(&at, 256 * 1024);
    if (pthread_create(&th, &at, sn_io_worker, io) != 0) sn_fatal("<runtime>", "could not start a file operation thread");
    pthread_attr_destroy(&at);
    return t;
}

typedef struct { SnTask t; SnStr *result; } SnTaskStr;

static SnTask *sn_read_file(SnStr *path) { return sn_io_start(0, path, NULL, sizeof(SnTaskStr)); }
static SnTask *sn_write_file(SnStr *path, SnStr *data) { return sn_io_start(1, path, data, sizeof(SnTask)); }

static void sn_io_finish(SnIo *io) {
    SnTask *t = io->task;
    for (SnTask **p = &sn_io_tasks; *p; p = &(*p)->aux_next)
        if (*p == t) { *p = t->aux_next; break; }
    sn_io_pending--;
    if (io->err) {
        char buf[600];
        snprintf(buf, sizeof buf, "cannot %s '%s': %s", io->op ? "write" : "read", io->path, strerror(io->err));
        t->failed = 1;
        t->exc = sn_str_new(buf, (int64_t)strlen(buf));
        t->exc_loc = "<file operation>";
        t->exc_kind = 1;
    } else if (io->op == 0) {
        ((SnTaskStr *)t)->result = sn_str_new(io->result ? io->result : "", (int64_t)io->rlen);
    }
    free(io->path); free(io->data); free(io->result); free(io);
    t->io = NULL;
    sn_task_complete(t);
}

static SnTask *sn_sleep(double seconds) {
    SnTask *t = (SnTask *)sn_gc_alloc(sizeof(SnTask), 0);
    if (!(seconds > 0)) seconds = 0;
    t->kind = 1;
    t->state = SN_TS_BLOCKED;
    t->deadline_ns = sn_now_ns() + (int64_t)(seconds * 1e9);
    SnTask **p = &sn_timers;
    while (*p && (*p)->deadline_ns <= t->deadline_ns) p = &(*p)->aux_next;
    t->aux_next = *p;
    *p = t;
    return t;
}

/* Fire due timers and collect finished file operations. */
static void sn_poll_events(void) {
    if (sn_timers) {
        int64_t now = sn_now_ns();
        while (sn_timers && sn_timers->deadline_ns <= now) {
            SnTask *t = sn_timers;
            sn_timers = t->aux_next;
            t->aux_next = NULL;
            sn_task_complete(t);
        }
    }
    if (sn_io_pending) {
        pthread_mutex_lock(&sn_io_mu);
        SnIo *list = sn_io_done;
        sn_io_done = NULL;
        pthread_mutex_unlock(&sn_io_mu);
        while (list) {
            SnIo *next = list->next;
            sn_io_finish(list);
            list = next;
        }
    }
}

/* Nothing is ready: sleep until a timer is due or a file operation finishes.
 * Returns 0 when there is nothing left to wait for. */
static int sn_wait_events(void) {
    if (!sn_timers && !sn_io_pending) return 0;
    int64_t wait_ns = -1;
    if (sn_timers) {
        wait_ns = sn_timers->deadline_ns - sn_now_ns();
        if (wait_ns < 0) wait_ns = 0;
    }
    if (sn_io_pending) {
        struct timespec ts;
        clock_gettime(CLOCK_REALTIME, &ts);
        int64_t add = wait_ns < 0 ? 1000000000LL : wait_ns;
        int64_t ns = (int64_t)ts.tv_nsec + add % 1000000000LL;
        ts.tv_sec += add / 1000000000LL + ns / 1000000000LL;
        ts.tv_nsec = ns % 1000000000LL;
        pthread_mutex_lock(&sn_io_mu);
        if (!sn_io_done) pthread_cond_timedwait(&sn_io_cv, &sn_io_mu, &ts);
        pthread_mutex_unlock(&sn_io_mu);
    } else if (wait_ns > 0) {
        struct timespec ts;
        ts.tv_sec = wait_ns / 1000000000LL;
        ts.tv_nsec = wait_ns % 1000000000LL;
        nanosleep(&ts, NULL);
    }
    return 1;
}

/* Run tasks until `w` is done (or, with w == NULL, until nothing is left). */
static void sn_run_until(SnTask *w) {
    for (;;) {
        if (w && w->state == SN_TS_DONE) return;
        sn_poll_events();
        SnTask *t = sn_ready_pop();
        if (t) { sn_resume(t); continue; }
        if (w && w->state == SN_TS_DONE) return;
        if (!sn_wait_events()) {
            if (w) sn_fatal("<runtime>", "deadlock: awaiting a task that can never finish");
            return;
        }
    }
}

/* ---------- await ---------- */

static void sn_await(SnTask *w) {
    if (w->state != SN_TS_DONE) {
        if (sn_cur) {
            SnTask *me = sn_cur;
            me->wait_next = w->waiters;
            w->waiters = me;
            sn_block_current();
        } else {
            sn_run_until(w);
        }
    }
    w->observed = 1;
    if (w->failed) sn_throw_str(w->exc_loc, w->exc, w->exc_kind);
}

/* ---------- GC integration and program start/end ---------- */

static void sn_async_scan_roots(void) {
    uintptr_t page = (uintptr_t)sysconf(_SC_PAGESIZE);
    if (sn_cur) {
        /* On a task stack: the main stack is suspended at sn_main_sp. */
        if (sn_main_base > (uintptr_t)sn_main_sp) sngc_scan(sn_main_sp, (const void *)sn_main_base);
    }
    for (SnTask *t = sn_all_head; t; t = t->all_next) {
        if (t->stack && t != sn_cur && t->state != SN_TS_NEW)
            sngc_scan(t->sp, (const void *)((uintptr_t)t->stack + t->stack_size));
    }
    (void)page;
}

static void sn_async_init(void) {
    sn_main_base = sn_gc.stack_base;
    sn_gc_extra = sn_async_scan_roots;
    sn_gc_root(&sn_ready_head, sizeof sn_ready_head);
    sn_gc_root(&sn_ready_tail, sizeof sn_ready_tail);
    sn_gc_root(&sn_all_head, sizeof sn_all_head);
    sn_gc_root(&sn_timers, sizeof sn_timers);
    sn_gc_root(&sn_io_tasks, sizeof sn_io_tasks);
    sn_gc_root(&sn_unobserved, sizeof sn_unobserved);
    const char *e = getenv("SN_TASK_STACK_KB");
    if (e && *e) {
        long kb = strtol(e, NULL, 10);
        if (kb >= 64) sn_task_stack_bytes = (size_t)kb * 1024;
    }
}

/* At program end: let every remaining task finish, then report failures
 * that nobody awaited. */
static void sn_async_finish(void) {
    sn_run_until(NULL);
    int bad = 0;
    for (SnTask *t = sn_unobserved; t; t = t->unobs_next) {
        if (t->observed) continue;
        fflush(stdout);
        fprintf(stderr, "%s: %s\n  at %s\n  (in a task that was never awaited)\n",
                t->exc_kind ? "RuntimeError" : "Error", t->exc->data, t->exc_loc);
        bad = 1;
    }
    if (bad) exit(1);
}

#endif /* SNASYNC_H */
