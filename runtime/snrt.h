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

#define SN_COLD __attribute__((cold, noreturn))

SN_COLD static void sn_panic(const char *loc, const char *fmt, ...) {
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
    if (!p) sn_panic("<runtime>", "out of memory");
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
        if (!b->p) sn_panic("<runtime>", "out of memory");
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

#endif /* SNRT_H */
