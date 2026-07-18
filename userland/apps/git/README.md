# git — distributed version control

This directory cross-builds git 2.52.0 as two fully static musl, non-PIE
executables for AgenticOS:

- `build/git` → `GIT.ELF` → `/bin/git` — every builtin command in one
  binary (`SKIP_DASHED_BUILT_INS`); no curl linkage.
- `build/git-remote-http` → `GITRHTTP.ELF` → `/bin/git-remote-http` and
  `/bin/git-remote-https` — the HTTP(S) transport helper, linked against
  static libcurl + OpenSSL 3.5.7. One ELF; the scheme comes from
  `argv[0]`, which the virtual `/bin` namespace preserves.

```sh
make -C userland/apps/git
REBUILD_GIT=1 ./build.sh -n
```

The git, curl, zlib, and OpenSSL archives and SHA256 values are pinned in
`Makefile`. The zlib, OpenSSL, and libcurl recipes are byte-for-byte the
qualified profile from `../curl/Makefile` (static, single-threaded TLS,
`OPENSSLDIR=/etc/ssl`, CA bundle `/etc/ssl/cert.pem`, IPv4 HTTP/HTTPS
only) — if yet another OpenSSL consumer appears, factor the shared
dependency build out rather than copying it again.

Build details that are load-bearing:

- The build host is macOS, so `uname_S=Linux uname_M=x86_64 …` overrides
  on the make command line force git's Linux platform block; without them
  git configures for Darwin.
- musl has no `REG_STARTEND`, so `NO_REGEX=NeedsStartEnd` selects git's
  bundled compat regex (the same choice Alpine makes).
- `gitexecdir=/bin` is the compiled-in exec path: git spawns
  `/bin/git-remote-https` for HTTPS remotes, and the kernel's bin
  namespace rewrite loads `GITRHTTP.ELF`. `sysconfdir=/etc` points the
  system config at the kernel-managed `/etc/gitconfig`.
- Compiled out: perl/python/tcl porcelain, gettext, iconv, expat (only
  legacy dumb-HTTP WebDAV push needs it), pthreads (pack/delta
  parallelism only — pthread groups pin to one CPU today), unix sockets
  (credential-cache), and IPv6. `git://` daemon URLs are out of scope;
  ssh remotes fail cleanly (no ssh client exists).

The kernel seeds `/etc/gitconfig` at boot (`src/userland/etc.rs`) with a
deterministic root identity, `init.defaultBranch=main`,
`safe.directory=*`, `core.fileMode=false` (FAT lower layer has no exec
bit), and `core.pager=cat`. Repos belong on `/work` (scratch) or `/data`
(persistent ext2); cwd starts at the read-only `/host`.

Inside a terminal:

```sh
cd /work && git init t && cd t
echo hi > f && git add f && git commit -m 'first'
git log --oneline
git clone https://github.com/octocat/Hello-World.git /work/hw
```

HTTPS certificate verification is strict by default against the
kernel-managed trust store; `git -c http.sslVerify=false` is the
explicit, user-typed escape hatch.

Licenses: `GIT-LICENSE.txt` (GPLv2), `CURL-LICENSE.txt` (curl, MIT-like)
and `OPENSSL-LICENSE.txt` (Apache-2.0) are copied from the pinned
upstream sources.
