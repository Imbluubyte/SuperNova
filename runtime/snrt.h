/* SuperNova runtime (single-header, included into every generated C file).
 *
 * Kept as one translation unit with the generated code so clang can inline
 * the small helpers. Plain C99 + GNU statement expressions (clang/gcc).
 * Memory: sn_alloc currently never frees (collector is on the roadmap). */
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

static inline void *sn_alloc(size_t n) {
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
    SnStr *r = (SnStr *)sn_alloc(sizeof(SnStr) + (size_t)len + 1);
    r->len = len;
    memcpy(r->data, s, (size_t)len);
    r->data[len] = 0;
    return r;
}

static inline SnStr *sn_str_concat(SnStr *a, SnStr *b) {
    SnStr *r = (SnStr *)sn_alloc(sizeof(SnStr) + (size_t)(a->len + b->len) + 1);
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
    b->p = (char *)sn_alloc((size_t)b->cap);
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

#endif /* SNRT_H */
