/* SuperNova runtime (single-header, included into every generated C file).
 *
 * Kept as one translation unit with the generated code so clang can inline
 * the small helpers. Plain C99 + GNU statement expressions (clang/gcc).
 * Memory: all strings and lists live in the garbage-collected heap (sngc.h). */
#ifndef SNRT_H
#define SNRT_H

#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <inttypes.h>
#include <math.h>
#include <errno.h>
#include <setjmp.h>

#define SN_COLD __attribute__((cold, noreturn))

/* Fatal errors cannot be caught by try/catch (out of memory, internal failures). */
SN_COLD static void sn_fatal(const char *loc, const char *fmt, ...) {
    fflush(stdout);
    va_list ap;
    va_start(ap, fmt);
    fprintf(stderr, "RuntimeError: ");
    vfprintf(stderr, fmt, ap);
    fprintf(stderr, "\n  at %s\n", loc);
    va_end(ap);
    exit(1);
}

#include "sngc.h"

/* Plain malloc for runtime-internal scratch memory that is not part of the
 * garbage-collected heap (e.g. string builders). */
static inline void *sn_xmalloc(size_t n) {
    void *p = malloc(n ? n : 1);
    if (!p) sn_fatal("<runtime>", "out of memory");
    return p;
}

/* ---------- strings: immutable, length-prefixed UTF-8 ---------- */

typedef struct SnStr {
    int64_t len;
    char data[];
} SnStr;

static inline SnStr *sn_str_new(const char *s, int64_t len) {
    SnStr *r = (SnStr *)sn_gc_alloc(sizeof(SnStr) + (size_t)len + 1, 1);
    r->len = len;
    memcpy(r->data, s, (size_t)len);
    r->data[len] = 0;
    return r;
}

/* ---------- errors: try / catch / throw ---------- */

/* Handlers form a stack of setjmp frames. A throw pops the innermost frame and
 * jumps to it. Each async task has its own stack of handlers. */
typedef struct SnTry { jmp_buf jb; struct SnTry *prev; } SnTry;
static SnTry *sn_try_top;
static SnStr *sn_exc;           /* message of the error being thrown (a GC root) */
static const char *sn_exc_loc;  /* where it was thrown */
static int sn_exc_kind;         /* 0 = `throw`, 1 = runtime error */

SN_COLD static void sn_throw_str(const char *loc, SnStr *msg, int kind) {
    if (!sn_try_top) {
        fflush(stdout);
        fprintf(stderr, "%s: %s\n  at %s\n", kind ? "RuntimeError" : "Error", msg->data, loc);
        exit(1);
    }
    sn_exc = msg;
    sn_exc_loc = loc;
    sn_exc_kind = kind;
    SnTry *t = sn_try_top;
    sn_try_top = t->prev;
    longjmp(t->jb, 1);
}

/* A runtime error (bad index, division by zero, ...). Catchable. */
SN_COLD static void sn_panic(const char *loc, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    if (!sn_try_top) {
        fflush(stdout);
        fprintf(stderr, "RuntimeError: ");
        vfprintf(stderr, fmt, ap);
        fprintf(stderr, "\n  at %s\n", loc);
        va_end(ap);
        exit(1);
    }
    char buf[1024];
    int n = vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    if (n < 0) n = 0;
    if (n >= (int)sizeof buf) n = (int)sizeof buf - 1;
    sn_throw_str(loc, sn_str_new(buf, n), 1);
}

static inline SnStr *sn_str_concat(SnStr *a, SnStr *b) {
    SnStr *r = (SnStr *)sn_gc_alloc(sizeof(SnStr) + (size_t)(a->len + b->len) + 1, 1);
    r->len = a->len + b->len;
    memcpy(r->data, a->data, (size_t)a->len);
    memcpy(r->data + a->len, b->data, (size_t)b->len);
    r->data[r->len] = 0;
    return r;
}

static inline int sn_str_cmp(SnStr *a, SnStr *b) {
    int64_t n = a->len < b->len ? a->len : b->len;
    int c = memcmp(a->data, b->data, (size_t)n);
    if (c) return c;
    return a->len < b->len ? -1 : (a->len > b->len ? 1 : 0);
}

static inline bool sn_str_eq(SnStr *a, SnStr *b) {
    return a->len == b->len && memcmp(a->data, b->data, (size_t)a->len) == 0;
}

/* ---------- string builder (used for f-strings and conversions) ---------- */

typedef struct {
    char *p;
    int64_t len, cap;
} SnBuf;

static inline void sn_buf_init(SnBuf *b) {
    b->cap = 48;
    b->len = 0;
    b->p = (char *)sn_xmalloc((size_t)b->cap);
}

static inline void sn_buf_text(SnBuf *b, const char *s, int64_t n) {
    if (b->len + n > b->cap) {
        while (b->len + n > b->cap) b->cap *= 2;
        b->p = (char *)realloc(b->p, (size_t)b->cap);
        if (!b->p) sn_fatal("<runtime>", "out of memory");
    }
    memcpy(b->p + b->len, s, (size_t)n);
    b->len += n;
}

static int sn_fmt_float(char *out, double v) {
    int n;
    if (isnan(v)) return sprintf(out, "nan");
    if (isinf(v)) return sprintf(out, v < 0 ? "-inf" : "inf");
    n = 0;
    for (int prec = 15; prec <= 17; prec++) {
        n = snprintf(out, 40, "%.*g", prec, v);
        if (strtod(out, NULL) == v) break;
    }
    if (!strpbrk(out, ".eE")) {
        out[n++] = '.';
        out[n++] = '0';
        out[n] = 0;
    }
    return n;
}

static inline void sn_buf_int(SnBuf *b, int64_t v) {
    char t[32];
    int n = snprintf(t, sizeof t, "%" PRId64, v);
    sn_buf_text(b, t, n);
}
static inline void sn_buf_float(SnBuf *b, double v) {
    char t[48];
    int n = sn_fmt_float(t, v);
    sn_buf_text(b, t, n);
}
static inline void sn_buf_bool(SnBuf *b, bool v) {
    if (v) sn_buf_text(b, "true", 4); else sn_buf_text(b, "false", 5);
}
static inline void sn_buf_str(SnBuf *b, SnStr *s) { sn_buf_text(b, s->data, s->len); }
static inline void sn_buf_null(SnBuf *b, int8_t v) { (void)v; sn_buf_text(b, "null", 4); }

static inline SnStr *sn_buf_finish(SnBuf *b) {
    SnStr *r = sn_str_new(b->p, b->len);
    free(b->p);
    return r;
}

static inline SnStr *sn_int_to_str(int64_t v) {
    SnBuf b; sn_buf_init(&b); sn_buf_int(&b, v); return sn_buf_finish(&b);
}
static inline SnStr *sn_float_to_str(double v) {
    SnBuf b; sn_buf_init(&b); sn_buf_float(&b, v); return sn_buf_finish(&b);
}
static inline SnStr *sn_bool_to_str(bool v) {
    return v ? sn_str_new("true", 4) : sn_str_new("false", 5);
}
static inline SnStr *sn_null_to_str(int8_t v) { (void)v; return sn_str_new("null", 4); }

/* ---------- printing ---------- */

static inline void sn_print_int(int64_t v) { printf("%" PRId64, v); }
static inline void sn_print_float(double v) { char t[48]; sn_fmt_float(t, v); fputs(t, stdout); }
static inline void sn_print_bool(bool v) { fputs(v ? "true" : "false", stdout); }
static inline void sn_print_str(SnStr *s) { fwrite(s->data, 1, (size_t)s->len, stdout); }
static inline void sn_print_null(int8_t v) { (void)v; fputs("null", stdout); }
static inline void sn_print_space(void) { fputc(' ', stdout); }
static inline void sn_print_nl(void) { fputc('\n', stdout); }

/* ---------- arithmetic with defined semantics ----------
 * Int +,-,* wrap on overflow (compiled with -fwrapv).
 * Division and modulo by zero are runtime errors.
 * // and % use floor semantics (result of % has the sign of the divisor). */

static inline int64_t sn_idiv(int64_t a, int64_t b, const char *loc) {
    if (b == 0) sn_panic(loc, "division by zero");
    if (a == INT64_MIN && b == -1) sn_panic(loc, "integer overflow in division");
    int64_t q = a / b;
    if ((a % b != 0) && ((a < 0) != (b < 0))) q -= 1;
    return q;
}

static inline int64_t sn_imod(int64_t a, int64_t b, const char *loc) {
    if (b == 0) sn_panic(loc, "modulo by zero");
    if (b == -1) return 0;
    int64_t r = a % b;
    if (r != 0 && ((r < 0) != (b < 0))) r += b;
    return r;
}

static inline int64_t sn_ipow(int64_t a, int64_t b, const char *loc) {
    if (b < 0) sn_panic(loc, "negative exponent in Int power; use Float values");
    int64_t r = 1;
    while (b > 0) {
        if (b & 1) r *= a;
        a *= a;
        b >>= 1;
    }
    return r;
}

static inline double sn_fdiv(double a, double b, const char *loc) {
    if (b == 0.0) sn_panic(loc, "division by zero");
    return a / b;
}
static inline double sn_ffloordiv(double a, double b, const char *loc) {
    if (b == 0.0) sn_panic(loc, "division by zero");
    return floor(a / b);
}
static inline double sn_fmod(double a, double b, const char *loc) {
    if (b == 0.0) sn_panic(loc, "modulo by zero");
    return a - floor(a / b) * b;
}

/* ---------- conversions ---------- */

static inline int64_t sn_f2i(double v, const char *loc) {
    if (isnan(v) || v >= 9223372036854775808.0 || v < -9223372036854775808.0)
        sn_panic(loc, "cannot convert %g to Int (out of range)", v);
    return (int64_t)v;
}

static inline int64_t sn_str_to_int(SnStr *s, const char *loc) {
    char *end;
    errno = 0;
    long long v = strtoll(s->data, &end, 10);
    if (s->len == 0 || *end != 0 || errno == ERANGE || end != s->data + s->len)
        sn_panic(loc, "cannot convert \"%s\" to Int", s->data);
    return (int64_t)v;
}

static inline double sn_str_to_float(SnStr *s, const char *loc) {
    char *end;
    double v = strtod(s->data, &end);
    if (s->len == 0 || *end != 0 || end != s->data + s->len)
        sn_panic(loc, "cannot convert \"%s\" to Float", s->data);
    return v;
}


/* ---------- Any: tagged value used by mixed-type lists ---------- */

enum { SN_T_NULL, SN_T_INT, SN_T_FLOAT, SN_T_BOOL, SN_T_STR };

typedef struct SnVal {
    uint8_t tag;
    union { int64_t i; double f; bool b; SnStr *s; } u;
} SnVal;

static inline SnVal sn_val_int(int64_t v) { SnVal r; r.tag = SN_T_INT; r.u.i = v; return r; }
static inline SnVal sn_val_float(double v) { SnVal r; r.tag = SN_T_FLOAT; r.u.f = v; return r; }
static inline SnVal sn_val_bool(bool v) { SnVal r; r.tag = SN_T_BOOL; r.u.b = v; return r; }
static inline SnVal sn_val_str(SnStr *v) { SnVal r; r.tag = SN_T_STR; r.u.s = v; return r; }
static inline SnVal sn_val_null(int8_t v) { (void)v; SnVal r; r.tag = SN_T_NULL; r.u.i = 0; return r; }

static inline bool sn_val_eq(SnVal a, SnVal b) {
    if (a.tag == SN_T_INT && b.tag == SN_T_FLOAT) return (double)a.u.i == b.u.f;
    if (a.tag == SN_T_FLOAT && b.tag == SN_T_INT) return a.u.f == (double)b.u.i;
    if (a.tag != b.tag) return false;
    switch (a.tag) {
        case SN_T_NULL: return true;
        case SN_T_INT: return a.u.i == b.u.i;
        case SN_T_FLOAT: return a.u.f == b.u.f;
        case SN_T_BOOL: return a.u.b == b.u.b;
        default: return sn_str_eq(a.u.s, b.u.s);
    }
}

/* repr variants quote strings; used for elements inside lists and tuples */
static inline void sn_buf_str_repr(SnBuf *b, SnStr *s) {
    sn_buf_text(b, "\"", 1);
    sn_buf_text(b, s->data, s->len);
    sn_buf_text(b, "\"", 1);
}

static inline void sn_buf_val(SnBuf *b, SnVal v) {
    switch (v.tag) {
        case SN_T_NULL: sn_buf_text(b, "null", 4); break;
        case SN_T_INT: sn_buf_int(b, v.u.i); break;
        case SN_T_FLOAT: sn_buf_float(b, v.u.f); break;
        case SN_T_BOOL: sn_buf_bool(b, v.u.b); break;
        default: sn_buf_str(b, v.u.s); break;
    }
}
static inline void sn_buf_val_repr(SnBuf *b, SnVal v) {
    if (v.tag == SN_T_STR) sn_buf_str_repr(b, v.u.s); else sn_buf_val(b, v);
}

/* ---------- lists: 1-based, bounds-checked, reference semantics ---------- */

SN_COLD static void sn_list_oob(const char *loc, int64_t i, int64_t len) {
    if (i < 1 && i != 0)
        sn_panic(loc, "list index %" PRId64 " is out of range (length %" PRId64 "; indexes start at 1, negative indexing is not supported)", i, len);
    sn_panic(loc, "list index %" PRId64 " is out of range (length %" PRId64 "; indexes start at 1)", i, len);
}

/* ATOMIC = 1 when T holds no pointers, so the collector never scans the storage. */
#define SN_DEFINE_LIST(N, T, ATOMIC) \
typedef struct N { int64_t len, cap; T *data; } N; \
static inline N *N##_new(int64_t cap) { \
    N *l = (N *)sn_gc_alloc(sizeof(N), 0); \
    if (cap < 4) cap = 4; \
    T *d = (T *)sn_gc_alloc(sizeof(T) * (size_t)cap, ATOMIC); \
    l->len = 0; l->cap = cap; l->data = d; return l; } \
static inline void N##_push(N *l, T v) { \
    if (l->len == l->cap) { \
        int64_t nc = l->cap * 2; \
        T *nd = (T *)sn_gc_alloc(sizeof(T) * (size_t)nc, ATOMIC); \
        memcpy(nd, l->data, sizeof(T) * (size_t)l->len); \
        sn_gc_free(l->data); l->data = nd; l->cap = nc; } \
    l->data[l->len++] = v; } \
static inline T *N##_at(N *l, int64_t i, const char *loc) { \
    if (i < 1 || i > l->len) sn_list_oob(loc, i, l->len); return &l->data[i - 1]; } \
static inline void N##_clear(N *l) { l->len = 0; } \
static inline void N##_reverse(N *l) { \
    for (int64_t a = 0, b = l->len - 1; a < b; a++, b--) { T t = l->data[a]; l->data[a] = l->data[b]; l->data[b] = t; } } \
static inline N *N##_copy(N *l) { N *r = N##_new(l->len); memcpy(r->data, l->data, sizeof(T) * (size_t)l->len); r->len = l->len; return r; } \
static inline void N##_merge(N *l, N *o) { int64_t n = o->len; for (int64_t i = 0; i < n; i++) N##_push(l, o->data[i]); }

#define SN_DEFINE_LIST_NUM(N, T) \
static inline T N##_sum(N *l) { T s = 0; for (int64_t i = 0; i < l->len; i++) s += l->data[i]; return s; } \
static inline T N##_min(N *l, const char *loc) { \
    if (l->len == 0) sn_panic(loc, "min() of an empty list"); \
    T m = l->data[0]; for (int64_t i = 1; i < l->len; i++) if (l->data[i] < m) m = l->data[i]; return m; } \
static inline T N##_max(N *l, const char *loc) { \
    if (l->len == 0) sn_panic(loc, "max() of an empty list"); \
    T m = l->data[0]; for (int64_t i = 1; i < l->len; i++) if (l->data[i] > m) m = l->data[i]; return m; } \
static inline double N##_mean(N *l, const char *loc) { \
    if (l->len == 0) sn_panic(loc, "mean() of an empty list"); \
    double s = 0; for (int64_t i = 0; i < l->len; i++) s += (double)l->data[i]; return s / (double)l->len; } \
static int N##_cmp(const void *a, const void *b) { T x = *(const T *)a, y = *(const T *)b; return (x > y) - (x < y); } \
static inline void N##_sort(N *l) { qsort(l->data, (size_t)l->len, sizeof(T), N##_cmp); }

#define SN_DEFINE_LIST_STRSORT(N) \
static int N##_cmp(const void *a, const void *b) { return sn_str_cmp(*(SnStr *const *)a, *(SnStr *const *)b); } \
static inline void N##_sort(N *l) { qsort(l->data, (size_t)l->len, sizeof(SnStr *), N##_cmp); }

/* ---------- dictionaries: insertion-ordered open-addressing hash maps ---------- */

static inline uint64_t sn_hash_int(int64_t x) {
    uint64_t z = (uint64_t)x + 0x9e3779b97f4a7c15ULL;
    z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
    z = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
    return z ^ (z >> 31);
}
static inline uint64_t sn_hash_bool(bool b) { return b ? 0x51ULL : 0xa3ULL; }
static inline uint64_t sn_hash_str(SnStr *s) {
    uint64_t h = 1469598103934665603ULL;
    for (int64_t i = 0; i < s->len; i++) { h ^= (unsigned char)s->data[i]; h *= 1099511628211ULL; }
    return h ^ (h >> 29);
}
static inline bool sn_keq_int(int64_t a, int64_t b) { return a == b; }
static inline bool sn_keq_bool(bool a, bool b) { return a == b; }
static inline bool sn_keq_str(SnStr *a, SnStr *b) { return sn_str_eq(a, b); }

SN_COLD static void sn_kfail_int(const char *loc, int64_t k) { sn_panic(loc, "key %" PRId64 " is not in the dictionary", k); }
SN_COLD static void sn_kfail_bool(const char *loc, bool k) { sn_panic(loc, "key %s is not in the dictionary", k ? "true" : "false"); }
SN_COLD static void sn_kfail_str(const char *loc, SnStr *k) { sn_panic(loc, "key \"%s\" is not in the dictionary", k->data); }

/* Entries are kept in insertion order; `tab` maps hashes to entry index + 1
   (0 = empty slot, -1 = deleted). Removed entries are zeroed and skipped. */
#define SN_DEFINE_DICT(N, K, V, ATOMIC, HASH, KEQ, KFAIL) \
typedef struct N##_e { K k; V v; uint64_t h; bool live; } N##_e; \
typedef struct N { int64_t count, used, cap, tcap; N##_e *ent; int64_t *tab; } N; \
static inline void N##_alloc(N *d, int64_t cap) { \
    int64_t tc = 8; while (tc < cap * 2) tc <<= 1; \
    N##_e *e = (N##_e *)sn_gc_alloc(sizeof(N##_e) * (size_t)cap, ATOMIC); \
    int64_t *t = (int64_t *)sn_gc_alloc(sizeof(int64_t) * (size_t)tc, 1); \
    memset(t, 0, sizeof(int64_t) * (size_t)tc); \
    d->ent = e; d->tab = t; d->cap = cap; d->tcap = tc; } \
static inline N *N##_new(int64_t cap) { \
    N *d = (N *)sn_gc_alloc(sizeof(N), 0); if (cap < 4) cap = 4; \
    d->count = d->used = 0; N##_alloc(d, cap); return d; } \
static inline int64_t N##_slot(N *d, K k, uint64_t h) { \
    uint64_t m = (uint64_t)d->tcap - 1, i = h & m; \
    for (;;) { int64_t s = d->tab[i]; if (s == 0) return -1; \
        if (s > 0) { N##_e *e = &d->ent[s - 1]; if (e->h == h && KEQ(e->k, k)) return (int64_t)i; } \
        i = (i + 1) & m; } } \
static inline void N##_rebuild(N *d, int64_t cap) { \
    N##_e *oe = d->ent; int64_t *ot = d->tab; int64_t n = d->used; \
    N##_alloc(d, cap); d->used = 0; \
    for (int64_t j = 0; j < n; j++) { if (!oe[j].live) continue; \
        uint64_t m = (uint64_t)d->tcap - 1, i = oe[j].h & m; \
        while (d->tab[i] != 0) i = (i + 1) & m; \
        d->ent[d->used] = oe[j]; d->tab[i] = d->used + 1; d->used++; } \
    sn_gc_free(oe); sn_gc_free(ot); } \
static inline void N##_put(N *d, K k, V v) { \
    uint64_t h = HASH(k); int64_t s = N##_slot(d, k, h); \
    if (s >= 0) { d->ent[d->tab[s] - 1].v = v; return; } \
    if (d->used == d->cap) N##_rebuild(d, d->count * 2 >= d->cap ? d->cap * 2 : d->cap); \
    uint64_t m = (uint64_t)d->tcap - 1, i = h & m; \
    while (d->tab[i] > 0) i = (i + 1) & m; \
    N##_e *e = &d->ent[d->used]; e->k = k; e->v = v; e->h = h; e->live = true; \
    d->tab[i] = d->used + 1; d->used++; d->count++; } \
static inline V *N##_at(N *d, K k, const char *loc) { \
    int64_t s = N##_slot(d, k, HASH(k)); if (s < 0) KFAIL(loc, k); \
    return &d->ent[d->tab[s] - 1].v; } \
static inline V N##_getor(N *d, K k, V def) { \
    int64_t s = N##_slot(d, k, HASH(k)); return s < 0 ? def : d->ent[d->tab[s] - 1].v; } \
static inline bool N##_has(N *d, K k) { return N##_slot(d, k, HASH(k)) >= 0; } \
static inline void N##_remove(N *d, K k) { \
    int64_t s = N##_slot(d, k, HASH(k)); if (s < 0) return; \
    memset(&d->ent[d->tab[s] - 1], 0, sizeof(N##_e)); d->tab[s] = -1; d->count--; } \
static inline void N##_clear(N *d) { \
    memset(d->ent, 0, sizeof(N##_e) * (size_t)d->used); \
    memset(d->tab, 0, sizeof(int64_t) * (size_t)d->tcap); d->count = d->used = 0; } \
static inline void N##_merge(N *d, N *o) { \
    int64_t n = o->used; \
    for (int64_t i = 0; i < n && i < o->used; i++) if (o->ent[i].live) N##_put(d, o->ent[i].k, o->ent[i].v); }

/* ---------- standard library support: strings, math, random, time, system, files ---------- */

#include <ctype.h>
#include <time.h>
#include <unistd.h>

/* Strings are byte strings: length and indexes count bytes (UTF-8 code units);
 * upper/lower change ASCII letters only. Indexes are 1-based. */

SN_COLD static void sn_str_oob(const char *loc, int64_t i, int64_t len) {
    sn_panic(loc, "string index %" PRId64 " is out of range (length %" PRId64 "; indexes start at 1)", i, len);
}

/* `count` bytes starting at 1-based `start`, clamped to the string. */
static inline SnStr *sn_str_sub(SnStr *s, int64_t start, int64_t count) {
    if (start < 1) { count += start - 1; start = 1; }
    if (count < 0 || start > s->len) count = 0;
    if (start + count - 1 > s->len) count = s->len - start + 1;
    if (count <= 0) return sn_str_new("", 0);
    return sn_str_new(s->data + start - 1, count);
}

static inline SnStr *sn_str_at(SnStr *s, int64_t i, const char *loc) {
    if (i < 1 || i > s->len) sn_str_oob(loc, i, s->len);
    return sn_str_new(s->data + i - 1, 1);
}

/* chars `from..to` inclusive, like a range */
static inline SnStr *sn_str_slice(SnStr *s, int64_t from, int64_t to, const char *loc) {
    if (from < 1 || to > s->len || from > to + 1)
        sn_panic(loc, "slice %" PRId64 "..%" PRId64 " is out of range (string length %" PRId64 "; indexes start at 1)", from, to, s->len);
    return sn_str_sub(s, from, to - from + 1);
}

/* 1-based index of the first occurrence of `sub` at or after `from`, or 0. */
static inline int64_t sn_str_find(SnStr *s, SnStr *sub, int64_t from) {
    if (from < 1) from = 1;
    if (sub->len == 0) return from <= s->len + 1 ? from : 0;
    if (from > s->len) return 0;
    const char *p = s->data + from - 1;
    int64_t left = s->len - (from - 1);
    while (left >= sub->len) {
        const char *q = (const char *)memchr(p, sub->data[0], (size_t)(left - sub->len + 1));
        if (!q) return 0;
        if (memcmp(q, sub->data, (size_t)sub->len) == 0) return (int64_t)(q - s->data) + 1;
        left -= (int64_t)(q - p) + 1;
        p = q + 1;
    }
    return 0;
}

static inline bool sn_str_starts(SnStr *s, SnStr *p) { return p->len <= s->len && memcmp(s->data, p->data, (size_t)p->len) == 0; }
static inline bool sn_str_ends(SnStr *s, SnStr *p) { return p->len <= s->len && memcmp(s->data + s->len - p->len, p->data, (size_t)p->len) == 0; }

static inline SnStr *sn_str_upper(SnStr *s) {
    SnStr *r = sn_str_new(s->data, s->len);
    for (int64_t i = 0; i < r->len; i++) if (r->data[i] >= 'a' && r->data[i] <= 'z') r->data[i] -= 32;
    return r;
}
static inline SnStr *sn_str_lower(SnStr *s) {
    SnStr *r = sn_str_new(s->data, s->len);
    for (int64_t i = 0; i < r->len; i++) if (r->data[i] >= 'A' && r->data[i] <= 'Z') r->data[i] += 32;
    return r;
}
static inline SnStr *sn_str_trim(SnStr *s) {
    int64_t a = 0, b = s->len;
    while (a < b && isspace((unsigned char)s->data[a])) a++;
    while (b > a && isspace((unsigned char)s->data[b - 1])) b--;
    return sn_str_new(s->data + a, b - a);
}

static inline SnStr *sn_str_replace(SnStr *s, SnStr *from, SnStr *to, const char *loc) {
    if (from->len == 0) sn_panic(loc, "replace() needs a non-empty text to look for");
    SnBuf b;
    sn_buf_init(&b);
    int64_t pos = 1;
    for (;;) {
        int64_t i = sn_str_find(s, from, pos);
        if (!i) break;
        sn_buf_text(&b, s->data + pos - 1, i - pos);
        sn_buf_text(&b, to->data, to->len);
        pos = i + from->len;
    }
    sn_buf_text(&b, s->data + pos - 1, s->len - (pos - 1));
    SnStr *r = sn_str_new(b.p, b.len);
    free(b.p);
    return r;
}

static inline SnStr *sn_str_repeat(SnStr *s, int64_t n, const char *loc) {
    if (n < 0) sn_panic(loc, "repeat() count cannot be negative");
    if (s->len && n > ((int64_t)1 << 40) / s->len) sn_panic(loc, "repeat() result is too large");
    SnStr *r = (SnStr *)sn_gc_alloc(sizeof(SnStr) + (size_t)(s->len * n) + 1, 1);
    r->len = s->len * n;
    for (int64_t i = 0; i < n; i++) memcpy(r->data + i * s->len, s->data, (size_t)s->len);
    r->data[r->len] = 0;
    return r;
}

/* math */
static inline int64_t sn_floor_i(double x, const char *loc) { return sn_f2i(floor(x), loc); }
static inline int64_t sn_ceil_i(double x, const char *loc) { return sn_f2i(ceil(x), loc); }
static inline int64_t sn_round_i(double x, const char *loc) { return sn_f2i(round(x), loc); }
static inline int64_t sn_trunc_i(double x, const char *loc) { return sn_f2i(trunc(x), loc); }

/* random numbers: splitmix64, seeded from the clock unless seed() is called */
static uint64_t sn_rng_state;
static int sn_rng_seeded;
static inline uint64_t sn_rng_next(void) {
    if (!sn_rng_seeded) {
        struct timespec ts;
        clock_gettime(CLOCK_REALTIME, &ts);
        sn_rng_state = (uint64_t)ts.tv_nsec * 6364136223846793005ULL ^ (uint64_t)ts.tv_sec ^ ((uint64_t)getpid() << 32);
        sn_rng_seeded = 1;
    }
    uint64_t z = (sn_rng_state += 0x9e3779b97f4a7c15ULL);
    z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
    z = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
    return z ^ (z >> 31);
}
static inline double sn_random(void) { return (double)(sn_rng_next() >> 11) * (1.0 / 9007199254740992.0); }
static inline void sn_seed(int64_t s) { sn_rng_state = (uint64_t)s; sn_rng_seeded = 1; }
static inline int64_t sn_rand_int(int64_t lo, int64_t hi, const char *loc) {
    if (hi < lo) sn_panic(loc, "randInt(%" PRId64 ", %" PRId64 "): the upper bound is below the lower bound", lo, hi);
    uint64_t span = (uint64_t)hi - (uint64_t)lo + 1;
    if (span == 0) return (int64_t)sn_rng_next();
    return (int64_t)((uint64_t)lo + sn_rng_next() % span);
}

/* time */
static inline double sn_wall_time(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}
static inline double sn_mono_time(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}

/* system */
static int sn_argc;
static char **sn_argv;
static inline int64_t sn_arg_count(void) { return sn_argc > 0 ? sn_argc - 1 : 0; }
static inline SnStr *sn_arg(int64_t i, const char *loc) {
    if (i < 1 || i > sn_arg_count()) sn_panic(loc, "argument %" PRId64 " does not exist (%" PRId64 " given)", i, sn_arg_count());
    return sn_str_new(sn_argv[i], (int64_t)strlen(sn_argv[i]));
}
static inline void sn_write(SnStr *s) { fwrite(s->data, 1, (size_t)s->len, stdout); fflush(stdout); }
SN_COLD static void sn_exit(int64_t code) { fflush(stdout); fflush(stderr); exit((int)code); }
static inline SnStr *sn_getenv(SnStr *n) { const char *v = getenv(n->data); return sn_str_new(v ? v : "", v ? (int64_t)strlen(v) : 0); }
static inline bool sn_hasenv(SnStr *n) { return getenv(n->data) != NULL; }
static inline SnStr *sn_read_line(const char *loc) {
    char *line = NULL;
    size_t cap = 0;
    fflush(stdout);
    ssize_t n = getline(&line, &cap, stdin);
    if (n < 0) { free(line); sn_panic(loc, "end of input"); }
    while (n > 0 && (line[n - 1] == '\n' || line[n - 1] == '\r')) n--;
    SnStr *r = sn_str_new(line, (int64_t)n);
    free(line);
    return r;
}

/* files (synchronous; see readFile/writeFile for the async versions) */
static inline SnStr *sn_read_text(SnStr *path, const char *loc) {
    FILE *f = fopen(path->data, "rb");
    if (!f) sn_panic(loc, "cannot read '%s': %s", path->data, strerror(errno));
    size_t cap = 4096, n = 0;
    char *buf = (char *)sn_xmalloc(cap);
    for (;;) {
        if (n == cap) { cap *= 2; buf = (char *)realloc(buf, cap); if (!buf) sn_fatal("<runtime>", "out of memory"); }
        size_t r = fread(buf + n, 1, cap - n, f);
        n += r;
        if (r == 0) break;
    }
    int bad = ferror(f);
    fclose(f);
    if (bad) { free(buf); sn_panic(loc, "cannot read '%s'", path->data); }
    SnStr *s = sn_str_new(buf, (int64_t)n);
    free(buf);
    return s;
}
static inline void sn_write_text(SnStr *path, SnStr *text, bool append, const char *loc) {
    FILE *f = fopen(path->data, append ? "ab" : "wb");
    if (!f) sn_panic(loc, "cannot write '%s': %s", path->data, strerror(errno));
    size_t w = text->len ? fwrite(text->data, 1, (size_t)text->len, f) : 0;
    int bad = fclose(f) != 0 || w != (size_t)text->len;
    if (bad) sn_panic(loc, "cannot write '%s'", path->data);
}
static inline bool sn_file_exists(SnStr *path) { return access(path->data, F_OK) == 0; }

#endif /* SNRT_H */
