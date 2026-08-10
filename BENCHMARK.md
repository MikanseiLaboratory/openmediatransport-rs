# openmediatransport-rs / vmx-rs receive benchmarks

Target: **VMX1 progressive 8-bit 4:2:2 → BGRA**, 1920×1080 @ 60000/1001.

## Gates (plan)

| Metric | Gate |
|--------|------|
| VMX decode p99 | ≤ 12 ms |
| Receive → BGRA return p99 | ≤ 16.68 ms |
| 10-minute soak average FPS | ≥ 59.0 |
| Transport / decode drops (steady state) | 0 |
| Queue / memory | no unbounded growth |

## Local results (Windows x86_64, release + thin LTO)

Host: development PC (Windows). Recorded 2026-08-10.

### vmx-rs Criterion

```
vmx_decode_bgra_1080p_60000_1001
  time: ~3.09 ms mean  (was ~13.7 ms before fused slice→BGRA pack)
```

Well under the 12 ms decode gate.

### openmediatransport-rs loopback soak

Short soak (`OMT_SOAK_SECS=5`, release, post fused-pack):

```
sent=300 recv=300 fps=60.00
decode_peak_ms≈7.4 (steady, after 30-frame warmup)
drops_wire=0 drops_decode=0 age_peak_us≈0.5 ms
```

Criterion VMX decode mean ≈ **3.09 ms** (p99 gate 12 ms).

Full 10-minute gate:

```powershell
$env:OMT_SOAK_SECS=600
cargo test --release --test soak_loopback -- --nocapture
```

### Receiver architecture

- Video wire queue depth **3** (drop + `frames_dropped_wire`)
- Decoded video depth **1** latest-wins (`frames_dropped_decode` on overwrite)
- Audio queue **10**, metadata **60**
- Dedicated threads: video I/O, VMX decode, audio I/O
- Per-socket reconnect with 250 ms … 2 s exponential backoff
- FPA1 decode into a reusable planar buffer

## Multi-OS

Functional `cargo test` / `cargo test --release` should be run on Linux and macOS x86_64 CI
(or developer machines). This workspace run covered **Windows x86_64** only.
ISA paths (scalar / SSE4.1 runtime dispatch) are shared across the three desktop targets;
AVX2 remains an optional fast path in `vmx-rs`.
