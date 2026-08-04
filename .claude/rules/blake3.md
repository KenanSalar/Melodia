---
paths:
  - src/media/**/*.rs
  - src/tasks/**/*.rs
  - src/library/playlist_files.rs
  - src/library/playlist_files/**/*.rs
  - src/database/queries/**/*.rs
  - src/services/desktop_integration.rs
---

# BLAKE3 Best Practices

## File Hashing

- Prefer `hasher.update_reader(file)` over manual buffered reads — it uses an optimized internal buffer sized for SIMD operations
- `finalize()` is idempotent and non-consuming — can be called multiple times
- Output is 32 bytes by default; use `finalize_xof()` for extended/variable-length output

## Parallelism

- Do **not** use `update_rayon()` for inputs under 128 KiB — thread pool overhead dominates
- Avoid `update_mmap_rayon()` on spinning disks — random access patterns cause thrashing
- For music library scanning: parallelize at the **file level** with Rayon (`par_iter` over file paths), use single-threaded `update_reader()` per individual file
- `update_rayon()` is useful for single very large files (>1 MB) on SSDs

## Usage Pattern

```rust
// Single file hash
let mut hasher = blake3::Hasher::new();
let file = std::fs::File::open(path)?;
hasher.update_reader(file)?;
let hash = hasher.finalize();

// Parallel multi-file hashing
use rayon::prelude::*;
let hashes: Vec<_> = paths.par_iter()
    .map(|path| {
        let mut hasher = blake3::Hasher::new();
        let file = std::fs::File::open(path)?;
        hasher.update_reader(file)?;
        Ok(hasher.finalize().to_hex().to_string())
    })
    .collect();
```

## Output Formats

- `hash.to_hex()` — returns a `HexString` (array-backed, no heap allocation); call `.to_string()` for a `String`
- `hash.as_bytes()` — raw `&[u8; 32]` for binary storage or comparison
- For database storage, prefer `to_hex().to_string()` (64-char ASCII) or `as_bytes()` as BLOB

## Why BLAKE3 over SHA-2

- ~6x faster than SHA-256 on a single thread
- Scales with available parallelism (SIMD + multithreading)
- Cryptographically secure (not just a fast hash)
- No length-extension attacks (unlike SHA-256)

## Incremental Hashing

- `hasher.update(bytes)` — hash data in chunks without reading the entire file into memory
- `finalize()` is non-consuming and idempotent — call multiple times without re-hashing
- For streaming pipelines, feed chunks to the hasher as they arrive rather than buffering the whole input
