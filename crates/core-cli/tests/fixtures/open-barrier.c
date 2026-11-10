/* Development-only Linux race fixture. Never linked into the Core runtime. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

int open64(const char *path, int flags, ...) {
    mode_t mode = 0;
    int needs_mode = flags & O_CREAT;
#ifdef O_TMPFILE
    needs_mode |= (flags & O_TMPFILE) == O_TMPFILE;
#endif
    if (needs_mode) {
        va_list arguments;
        va_start(arguments, flags);
        mode = va_arg(arguments, mode_t);
        va_end(arguments);
    }
    int (*next_open)(const char *, int, ...) = dlsym(RTLD_NEXT, "open64");
    if (!next_open) _exit(120);
    const char *target = getenv("NUMERA_TEST_OPEN_TARGET");
    const char *ready = getenv("NUMERA_TEST_OPEN_READY");
    const char *resume = getenv("NUMERA_TEST_OPEN_RESUME");
    if (target && ready && resume && !strcmp(path, target)) {
        int signal = next_open(ready, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
        if (signal < 0) _exit(121);
        close(signal);
        int attempts = 0;
        while (access(resume, F_OK) != 0) {
            if (++attempts > 10000) _exit(122);
            usleep(1000);
        }
    }
    return needs_mode ? next_open(path, flags, mode) : next_open(path, flags);
}
