#include <stdio.h>
#include <dlfcn.h>
#include <string.h>
#include <stdlib.h>
#include <pthread.h>
static int64_t (*g_add)(int64_t);

static void hostLog(const char *m) { printf("[host] %s\n", m); }
static double hostTime(void) { return 12.5; }

static void *worker(void *arg) {
	printf("thread: addScore=%ld\n", (long)g_add(1000));
	return 0;
}

int main(void) {
	void *h = dlopen("./libgame.so", RTLD_NOW);
	if (!h) { printf("dlopen failed: %s\n", dlerror()); return 1; }
	int (*init)(void) = dlsym(h, "sn_init");
	/* calling before init fails cleanly */
	int64_t (*addScore)(int64_t) = dlsym(h, "sn_addScore"); g_add = addScore;
	const char *(*lasterr)(void) = dlsym(h, "sn_last_error");
	printf("before init: %ld err=[%s]\n", (long)addScore(1), lasterr());
	void (*setlog)(void *) = dlsym(h, "sn_set_extern_hostLog");
	void (*settime)(void *) = dlsym(h, "sn_set_extern_hostTime");
	setlog(hostLog); settime(hostTime);
	printf("init=%d\n", init());
	void (*start)(const char *) = dlsym(h, "sn_start");
	start("sword");
	printf("addScore=%ld\n", (long)addScore(5));
	printf("addScore=%ld\n", (long)addScore(7));
	const char *(*describe)(void) = dlsym(h, "sn_describe");
	printf("%s\n", describe());
	double (*half)(double) = dlsym(h, "sn_half");
	printf("half=%g\n", half(5.0));
	int32_t (*isr)(void) = dlsym(h, "sn_isRunning");
	printf("running=%d\n", isr());
	int64_t (*boom)(int64_t) = dlsym(h, "sn_boom");
	printf("boom(2)=%ld\n", (long)boom(2));
	int64_t r = boom(9);
	printf("boom(9)=%ld err=[%s]\n", (long)r, lasterr());
	printf("after error addScore=%ld err=[%s]\n", (long)addScore(1), lasterr());
	int64_t (*churn)(int64_t) = dlsym(h, "sn_churn");
	printf("churn=%ld\n", (long)churn(300000));
	pthread_t t; pthread_create(&t, 0, worker, 0); pthread_join(t, 0);
	printf("churn from main again=%ld\n", (long)churn(100000));
	printf("final: %s\n", describe());
	return 0;
}
