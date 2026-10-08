# Collaboration schema artifacts

The release archive contains the shared type catalog, CLI output schemas, and
the exact pinned ACP schema plus its upstream license. Rust owns the
collaboration types. ACP definitions are copied from the pinned upstream schema
when composing CLI conversation output schemas; they are not maintained as a
second method/type registry.

The collaboration API's tools publish their own input and output schemas through
MCP `tools/list` on the running Host; the release archive does not freeze a copy.

`communication-control/protocol-types.json` contains named DTO and CLI-envelope
schemas. `FiniteCommandRecord` is generic: specialize its result/error payload
with the relevant tool's output schema. `NativeObservationRecord.message`
preserves an opaque native object. `ConversationRecord` includes namespaced
definitions from the pinned ACP schema and resolves them offline.

DTO native references such as `urn:codex-native:ThreadStatus` are templates. For
native-aware generation, use the running Host's native schema bundle, whose
digest the Host advertises as `nativeSchemaDigest` in its owner-private service
manifest. The native bundle is exported from the executable actually serving the
captured backend; release packaging does not invent or freeze a different native
method registry.

Generate the static type catalog without starting Codex or accessing session
state:

```sh
cargo run -p collaboration-protocol --example export_protocol_types
```

Schema-only artifacts do not constitute Swift, TypeScript or Python client
implementations; V1 ships the Rust client and CLI, with other language clients
following the same contract.
