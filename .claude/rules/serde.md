---
paths:
  - src/entities/**/*.rs
  - src/services/**/*.rs
  - src/library/settings/**/*.rs
  - src/state/**/*.rs
---

# Serde Best Practices

## Derive vs Manual

- Always prefer `#[derive(Serialize, Deserialize)]` — manual impls are error-prone and rarely needed
- Use `#[serde(with = "module")]` or `#[serde(deserialize_with = "fn")]` for field-level customization
- Use `#[serde(remote = "ExternalType")]` for types you don't own

## Key Field Attributes

- `#[serde(rename = "name")]` — rename a single field
- `#[serde(skip_serializing_if = "Option::is_none")]` — omit None fields from output
- `#[serde(default)]` — use Default::default() if field is missing during deserialization
- `#[serde(skip)]` — exclude field from both serialization and deserialization
- `#[serde(flatten)]` — inline a nested struct's fields into the parent
- `#[serde(borrow)]` — enable zero-copy deserialization for `&str` and `Cow<str>` fields

## Key Container Attributes

- `#[serde(rename_all = "camelCase")]` — rename all fields (useful for JSON APIs)
- `#[serde(deny_unknown_fields)]` — reject input with unexpected fields (strict parsing)

## Enum Representations (by performance)

1. **Externally tagged** (default) — fastest: `{"variant": { ...data }}`
2. **Adjacently tagged** `#[serde(tag = "type", content = "data")]` — `{"type":"v", "data":{...}}`
3. **Internally tagged** `#[serde(tag = "type")]` — ~2x slower, buffers input
4. **Untagged** `#[serde(untagged)]` — slowest, tries each variant sequentially; buffers entire input

## Zero-Copy Deserialization

- `&'a str` for guaranteed borrow from input (fails if escaping needed)
- `Cow<'a, str>` for best-effort borrow (borrows when possible, allocates when escapes needed)
- `String` for always-owned (safe but allocates)
- Zero-copy only works with `serde_json::from_str` / `from_slice`, **not** `from_reader`

## Trait Bounds

- `Deserialize<'de>` — when the caller provides the data and controls its lifetime
- `DeserializeOwned` — when the function manages data internally (equivalent to `for<'de> Deserialize<'de>`)

## Performance Tips

- Avoid `#[serde(untagged)]` enums in hot paths — each variant is tried sequentially
- Use `#[serde(flatten)]` sparingly — it disables some optimizations
- For large binary data, consider raw bytes or base64 with a custom serializer rather than JSON arrays
- Prefer `serde_json::from_str` over `from_reader` when data is already in memory — enables zero-copy

## Additional Field & Variant Attributes

- `#[serde(alias = "other_name")]` — accept alternative names during deserialization (useful for API compatibility)
- `#[serde(serialize_with = "fn")]` / `#[serde(deserialize_with = "fn")]` — per-field custom logic without a full `with` module
- `#[serde(skip_serializing)]` / `#[serde(skip_deserializing)]` — one-directional skip
- `#[serde(default = "path::to::fn")]` — call a specific function for the default value (not just `Default::default()`)
- `#[serde(bound = "T: Serialize")]` — override the auto-generated trait bounds on derived impls

## Container Attributes (Additional)

- `#[serde(transparent)]` — newtype wrapper that serializes as the inner type, no outer struct layer
- `#[serde(rename_all_fields = "camelCase")]` — applies `rename_all` to all fields of all enum variants (Serde 1.0.171+)
- `#[serde(from = "OtherType")]` / `#[serde(into = "OtherType")]` — implement via `From`/`Into` conversions instead of manual visitors

## Error Handling & Compatibility

- Missing fields with `#[serde(default)]` silently fill defaults — use `#[serde(deny_unknown_fields)]` if strict validation is needed
- Combine `#[serde(default)]` + `#[serde(skip_serializing_if = "...")]` for optional forward/backward compatible fields
- For schema evolution: adding fields with `#[serde(default)]` is backward compatible; removing fields requires `deny_unknown_fields` to be absent

## Custom Serialization Patterns

- For `#[serde(with = "module")]`, the module must expose `serialize` and `deserialize` fns with exact signatures
- Use `#[serde(remote = "ExternalType")]` + a local newtype to serialize foreign types without orphan rule violations
- Implement `Serialize` manually only when derived output must differ structurally (e.g. flattening a field conditionally)
