#include <stdio.h>
#include <stdint.h>
#include <dlfcn.h>
#define F(T, name, ...) T (*name)(__VA_ARGS__) = dlsym(h, #name); if (!name) { printf("missing %s\n", #name); return 1; }

static int quiet;
static void hostSetPos(int64_t id, double x, double y) { if (!quiet) printf("[host] node %ld -> (%g, %g)\n", (long)id, x, y); }

int main(void) {
	void *h = dlopen("./libnodes.so", RTLD_NOW);
	if (!h) { printf("%s\n", dlerror()); return 1; }
	F(int, sn_init, void)
	F(const char *, sn_last_error, void)
	F(void, sn_release, int64_t)
	F(void, sn_set_extern_hostSetPos, void *)
	F(int64_t, sn_new_Player, int64_t, const char *)
	F(int64_t, sn_new_Node2D, int64_t, const char *)
	F(void, sn_Player_onUpdate, int64_t, double)
	F(void, sn_Node2D_onUpdate, int64_t, double)
	F(const char *, sn_Player_describe, int64_t)
	F(int32_t, sn_Player_hurt, int64_t, int64_t)
	F(double, sn_Player_get_x, int64_t)
	F(void, sn_Player_set_speed, int64_t, double)
	F(const char *, sn_Node2D_get_kind, int64_t)
	F(void, sn_Player_explode, int64_t)
	sn_set_extern_hostSetPos(hostSetPos);
	printf("init=%d\n", sn_init());
	int64_t p = sn_new_Player(7, "hero");
	int64_t n = sn_new_Node2D(8, "plain");
	printf("handles %ld %ld\n", (long)p, (long)n);
	sn_Player_set_speed(p, 50.0);
	sn_Player_onUpdate(p, 0.5);
	sn_Node2D_onUpdate(n, 0.5);
	printf("%s | %s\n", sn_Player_describe(p), sn_Player_describe(p));
	printf("x=%g kind=%s\n", sn_Player_get_x(p), sn_Node2D_get_kind(p));
	int32_t h1 = sn_Player_hurt(p, 1); int32_t h2 = sn_Player_hurt(p, 9);
	printf("hurt=%d hurt=%d\n", h1, h2);
	sn_Player_explode(p);
	printf("err=[%s]\n", sn_last_error());
	/* a plain Node2D handle is not a Player */
	sn_Player_describe(n);
	printf("err=[%s]\n", sn_last_error());
	sn_release(p);
	sn_Player_describe(p);
	printf("err=[%s]\n", sn_last_error());
	/* many objects: handles are reused and GC keeps live ones */
	quiet = 1;
	int64_t keep = sn_new_Player(1, "keeper");
	for (int i = 0; i < 20000; i++) { int64_t t = sn_new_Player(i, "temp"); sn_Player_onUpdate(t, 0.1); sn_release(t); }
	printf("keeper: %s\n", sn_Player_describe(keep));
	printf("reused handle: %ld\n", (long)sn_new_Player(2, "again"));
	return 0;
}
