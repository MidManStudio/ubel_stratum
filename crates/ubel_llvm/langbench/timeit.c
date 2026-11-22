/* timeit STATS_FILE command [args...]
 * Runs the command once (its stdout and stderr pass through) and writes one line to STATS_FILE:
 *   wall_ms user_ms sys_ms maxrss_kb exit_status
 * Portable (Linux, macOS, Termux): fork, execvp, wait4. Not a benchmark itself; it only measures one child. */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>
#include <sys/resource.h>
#include <sys/time.h>
#include <sys/wait.h>

static double now_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1000.0 + t.tv_nsec / 1.0e6;
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: timeit STATS_FILE command [args...]\n");
        return 2;
    }
    double t0 = now_ms();
    pid_t pid = fork();
    if (pid < 0) { perror("fork"); return 2; }
    if (pid == 0) {
        execvp(argv[2], argv + 2);
        perror(argv[2]);
        _exit(127);
    }
    int status = 0;
    struct rusage ru;
    if (wait4(pid, &status, 0, &ru) < 0) { perror("wait4"); return 2; }
    double wall = now_ms() - t0;
    double user = ru.ru_utime.tv_sec * 1000.0 + ru.ru_utime.tv_usec / 1000.0;
    double sys = ru.ru_stime.tv_sec * 1000.0 + ru.ru_stime.tv_usec / 1000.0;
    long rss = ru.ru_maxrss;
#ifdef __APPLE__
    rss /= 1024; /* bytes on macOS, kilobytes elsewhere */
#endif
    int code = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
    FILE *f = fopen(argv[1], "w");
    if (!f) { perror(argv[1]); return 2; }
    fprintf(f, "%.1f %.1f %.1f %ld %d\n", wall, user, sys, rss, code);
    fclose(f);
    return 0;
}
