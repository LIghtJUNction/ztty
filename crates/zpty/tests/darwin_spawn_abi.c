#define _DARWIN_C_SOURCE 1
#include <errno.h>
#include <fcntl.h>
#include <libproc.h>
#include <poll.h>
#include <signal.h>
#include <spawn.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/ioctl.h>
#include <sys/proc_info.h>
#include <sys/ttycom.h>
#include <sys/wait.h>
#include <unistd.h>

_Static_assert(sizeof(posix_spawnattr_t) == sizeof(void *), "spawn attr ABI");
_Static_assert(sizeof(posix_spawn_file_actions_t) == sizeof(void *), "actions ABI");
_Static_assert(sizeof(sigset_t) == 4 && sizeof(mode_t) == 2, "Darwin scalar ABI");
_Static_assert(sizeof(nfds_t) == 4 && sizeof(struct pollfd) == 8, "poll ABI");
_Static_assert(offsetof(struct pollfd, events) == 4, "poll events offset");
_Static_assert(offsetof(struct pollfd, revents) == 6, "poll revents offset");
_Static_assert(sizeof(struct winsize) == 8, "winsize ABI");
_Static_assert(offsetof(struct winsize, ws_col) == 2, "winsize columns offset");
_Static_assert(offsetof(struct winsize, ws_xpixel) == 4, "winsize pixels offset");
_Static_assert(O_RDWR == 2 && O_NONBLOCK == 4, "open modes");
_Static_assert(O_NOCTTY == 0x20000 && O_CLOEXEC == 0x1000000, "open flags");
_Static_assert(F_DUPFD_CLOEXEC == 67 && FD_CLOEXEC == 1, "descriptor flags");
_Static_assert(AT_FDCWD == -2 && AT_EACCESS == 0x10 && X_OK == 1,
    "effective-identity executable access");
_Static_assert(F_GETFD == 1 && F_SETFD == 2 && F_GETFL == 3 && F_SETFL == 4,
    "fcntl commands");
_Static_assert(TIOCPTYGNAME == 0x40807453 && TIOCSCTTY == 0x20007461, "tty ioctls");
_Static_assert(TIOCSWINSZ == 0x80087467 && TIOCGWINSZ == 0x40087468, "size ioctls");
_Static_assert(POSIX_SPAWN_SETSID == 0x400, "setsid flag");
_Static_assert(POSIX_SPAWN_CLOEXEC_DEFAULT == 0x4000, "inheritance flag");
_Static_assert(POSIX_SPAWN_SETSIGDEF == 4 && POSIX_SPAWN_SETSIGMASK == 8,
    "signal flags");
_Static_assert(SIGKILL == 9 && SIGSTOP == 17 && SIGHUP == 1, "signals");
_Static_assert(WNOHANG == 1 && PROC_PIDPATHINFO_MAXSIZE == 4096, "process ABI");
_Static_assert(EINTR == 4 && EWOULDBLOCK == 35 && ETIMEDOUT == 60, "retry errors");
_Static_assert(EIO == 5 && EINVAL == 22 && ECHILD == 10, "protocol/reap errors");

int (*const zpty_spawn_signature)(pid_t *, const char *,
    const posix_spawn_file_actions_t *, const posix_spawnattr_t *,
    char *const [], char *const []) = posix_spawn;
int (*const zpty_flags_signature)(posix_spawnattr_t *, short) = posix_spawnattr_setflags;
int (*const zpty_open_signature)(posix_spawn_file_actions_t *, int,
    const char *, int, mode_t) = posix_spawn_file_actions_addopen;
int (*const zpty_path_signature)(int, void *, uint32_t) = proc_pidpath;
int (*const zpty_child_pids_signature)(pid_t, void *, int) = proc_listchildpids;
int (*const zpty_access_signature)(int, const char *, int, int) = faccessat;
