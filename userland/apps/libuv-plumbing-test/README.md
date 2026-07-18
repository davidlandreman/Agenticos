# Static-musl libuv plumbing fixture

`UVPLUMB.ELF` exercises the Linux primitives used by libuv's Linux backend:
legacy and modern eventfd/epoll entry points, edge-triggered delivery,
AF_UNIX stream socket pairs, scheduler yield, alternate signal stacks,
anonymous-memory advice/remapping, and private-expedited membarrier.

The source builds as a static non-PIE musl `ET_EXEC`. Refresh the committed
test fixture through the manifest-driven workflow after changing it:

```sh
./userland/refresh-prebuilt.sh
```

Ordinary test runs stage `userland/prebuilt/libuv/UVPLUMB.ELF`, so they do not
require the cross-compiler.

Committed fixture provenance: 9,168 bytes, SHA-256
`4cefeae3b752603f1b5208bcc589b5ae7c3eafc537daf1dd19f9de8e20d7386f`.
