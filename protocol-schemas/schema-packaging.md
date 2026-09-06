# Communication schema artifacts

The release archive contains the versioned Control schema, shared type catalog,
CLI output schemas, and the exact pinned ACP schema plus its upstream license.
Rust owns Control types and method pairings. ACP definitions are copied from the
pinned upstream schema when composing CLI conversation output schemas; they are
not maintained as a second method/type registry.

`communication-control/control-schema.json` is the **unbound** Control profile.
Metadata methods and message types remain defined. Native-dependent response
shapes are unavailable until a specific native bundle is selected. Do not use its
digest as the identity of a running Host's bound schema.

`communication-control/protocol-types.json` contains named DTO and CLI-envelope
schemas. `FiniteCommandRecord` is generic: specialize its result/error payload
with the relevant Control method's schema. `NativeObservationRecord.message`
preserves an opaque native object. `ConversationRecord` includes namespaced
definitions from the pinned ACP schema and resolves them offline.

DTO native references such as `urn:codex-native:ThreadStatus` are templates. For
native-aware generation, use the running service's bound Control schema and
native schema bundle. The Host publishes `control-schema-<digest>.json` before
advertising that digest in its owner-private service manifest. The native bundle
is exported from the executable actually serving the captured backend; release
packaging does not invent or freeze a different native method registry.

Generate the static artifacts without starting Codex or accessing session state:

```sh
cargo run -p communication-protocol --example export_control_schema
cargo run -p communication-protocol --example export_protocol_types
```

The first command writes canonical JSON to stdout and its digest to stderr. It
also accepts an explicitly selected native digest when a bound schema is needed.
The second writes the named type catalog. Schema-only artifacts do not constitute
Swift, TypeScript or Python client implementations; V1 ships the Rust client and
CLI, with other language clients following the same contract.
