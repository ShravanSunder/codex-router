# Native F4 in the private capture tool

The pinned VHS0.12.1 parser has no function-key command. `pinned-vhs-f4.patch`
adds F4 at the existing token, parser and native-key dispatch boundary. It does
not add another input transport or application shortcut. The upstream source is
`charmbracelet/vhs` commit `24fa2254a9806091e6ee6a980e9f3bcfe0a9ba53`.

Make an isolated source archive in an owned system temporary task directory;
keep the OSS checkout and original VHS binary unchanged. Copy
`parser_regression_test.go` into that archive's `parser/` directory. The test
fails on the original source because F4 is a STRING, before applying the patch.
Apply only this zero-context patch with `git apply --unidiff-zero`, then run the
parser, token and lexer packages. The executed build receipt retains the original
context-patch hash; this whitespace-clean representation produces the same three
modified files. Build with
the archive's unchanged `go.mod` and `go.sum`, `-mod=readonly`, one compiler job
and private Go caches. Label the resulting tool `0.12.1+private-f4`, bind its
upstream/patch/module/binary hashes, and nominate it in a separate capture-tools
root with the verified ttyd and libwebsockets artifacts. This is a private tool
repair, not an upstream release or global installation.

Go-rod0.116.2 defines native F4 as key/code F4 and virtual key115. The existing
VHS ExecuteKey path sends that event to the browser; textual escape-byte typing
would depend on input timing and is not used. Token/parser success cannot prove
delivery: the permanent compiled-TUI capture journeys require actual session
and quota details images and returning to the filtered list. Mapping F4 to
Enter or a no-op cannot satisfy those independent scene markers.

The observed Go dependency-fetch stall was a TCP dial wait, including after a
host grant; no TLS/auth cause was established. The bounded alternative fetched
only versions already listed in pinned `go.sum` through normally verified HTTPS
into a private file proxy. The build used that proxy for this command only,
kept checksum verification enabled and left both module files unchanged. No
global proxy, trust, account or app dependency setting changed.
