#include <stdio.h>
#include <stdint.h>
#include <dlfcn.h>
#include <time.h>
int main(void) {
	void *h = dlopen("./libtasks.so", RTLD_NOW);
	int (*init)(void) = dlsym(h, "sn_init");
	int (*pump)(void) = dlsym(h, "sn_pump");
	void (*start)(int64_t) = dlsym(h, "sn_startBlink");
	void (*sf)(void) = dlsym(h, "sn_startFailing");
	int64_t (*ticks)(void) = dlsym(h, "sn_getTicks");
	const char *(*lg)(void) = dlsym(h, "sn_lastLog");
	const char *(*err)(void) = dlsym(h, "sn_last_error");
	printf("init=%d\n", init());
	start(3);
	int frames = 0, p;
	struct timespec ts = {0, 5000000};
	while ((p = pump()) == 1 && frames < 1000) { frames++; nanosleep(&ts, 0); }
	printf("pump=%d ticks=%ld log=%s\n", p, (long)ticks(), lg());
	sf();
	do { p = pump(); nanosleep(&ts, 0); } while (p == 1);
	printf("pump=%d err=[%s]\n", p, err());
	printf("idle pump=%d\n", pump());
	return 0;
}
