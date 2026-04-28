// node:constants — legacy aggregate alias for fs/os/crypto constants.
// Deprecated in modern Node, still pulled in by some bundles.

(function () {
  module.exports = {
    O_RDONLY: 0, O_WRONLY: 1, O_RDWR: 2,
    O_CREAT: 0o100, O_EXCL: 0o200, O_NOCTTY: 0o400,
    O_TRUNC: 0o1000, O_APPEND: 0o2000,
    S_IFMT: 0o170000, S_IFREG: 0o100000, S_IFDIR: 0o040000,
    S_IFLNK: 0o120000, S_IFIFO: 0o010000,
    S_IRUSR: 0o400, S_IWUSR: 0o200, S_IXUSR: 0o100,
    F_OK: 0, R_OK: 4, W_OK: 2, X_OK: 1,
    SIGHUP: 1, SIGINT: 2, SIGQUIT: 3, SIGKILL: 9,
    SIGTERM: 15, SIGPIPE: 13,
    EPERM: 1, ENOENT: 2, EIO: 5, EBADF: 9,
    EACCES: 13, EEXIST: 17, ENOTDIR: 20, EISDIR: 21,
    EINVAL: 22, ENOSYS: 78,
    UV_DIRENT_FILE: 1, UV_DIRENT_DIR: 2, UV_DIRENT_LINK: 3,
  };
})();
