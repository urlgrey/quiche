# Finding 3: `max_tx_data - tx_data` Unsigned Subtraction Underflow

## Severity
High

## Bug Class
Integer Underflow

## Affected Code
- File: `quiche/src/lib.rs`
- Lines: 5913, 6412, 8810

## Description

Three locations in quiche perform `self.max_tx_data - self.tx_data` as unsigned `u64` arithmetic without checking that `max_tx_data >= tx_data`:

```rust
// Line 5913 (stream_do_send) — flow control blocked check
if self.max_tx_data - self.tx_data < len as u64 {

// Line 6412 (stream_writable) — writability check
if self.max_tx_data - self.tx_data < len as u64 {

// Line 8810 (update_tx_cap) — capacity calculation
let cap = cmp::min(cwin_available, self.max_tx_data - self.tx_data) as usize;
```

If `tx_data` ever exceeds `max_tx_data`, the unsigned subtraction wraps to a value near `u64::MAX`:

- **Math:** `0u64 - 1u64` in unsigned arithmetic = `18_446_744_073_709_551_615` (u64::MAX)
- At lines 5913/6412: The blocked check (`< len`) would never fire because `u64::MAX > len` for any reasonable len, so the connection appears **unblocked** even though it should be blocked
- At line 8810: `tx_cap` is set to the cwnd value (since `u64::MAX > cwin_available`), completely **bypassing the peer's flow control limit**

The `max_tx_data` field starts at 0 (line 2134) and is only updated when the peer's transport parameters arrive (line 7827). Any data queued via 0-RTT before transport parameters are processed could create a window where `tx_data > max_tx_data`.

## Reproduction Steps

The test demonstrates the underflow directly by manipulating internal state:

```bash
cargo test --package quiche --lib -- tests::tx_data_underflow_wraps_to_max --nocapture
```

Or run the standalone repro:

```bash
cd repro/
cargo test --nocapture
```

### Manual / Conceptual Steps:

1. Establish a QUIC connection with 0-RTT enabled
2. On the second connection (using session ticket), send data via 0-RTT before the server's transport parameters are processed
3. `tx_data` increments as data is buffered, but `max_tx_data` is still 0
4. The subtraction `max_tx_data(0) - tx_data(N)` wraps to `u64::MAX - N + 1`
5. All flow control checks are bypassed

## Expected vs Actual Behavior
- **Expected:** When `tx_data > max_tx_data`, the connection should be flow-control blocked. The subtraction should use `saturating_sub` or a checked comparison (`tx_data >= max_tx_data`).
- **Actual:** The unsigned subtraction wraps to ~`u64::MAX`, making the connection appear to have virtually unlimited flow control capacity. This bypasses the peer's `initial_max_data` transport parameter.

## Impact

An attacker can bypass connection-level flow control:

1. **Flow control bypass:** If a code path causes `tx_data` to exceed `max_tx_data` (even by 1 byte), the peer's flow control limit is completely bypassed. The attacker can send unlimited data.
2. **Memory exhaustion:** A peer expecting flow control to limit incoming data could have its receive buffers overwhelmed.
3. **0-RTT race window:** During 0-RTT connection establishment, there's a natural window where `max_tx_data = 0` but data may already be queued, creating the underflow condition.

The fix is straightforward: replace `self.max_tx_data - self.tx_data` with `self.max_tx_data.saturating_sub(self.tx_data)` at all three sites, or add an explicit guard `if self.tx_data >= self.max_tx_data`.

## References
- RFC 9000 §4.1 — Data Flow Control
- RFC 9000 §4.6 — Controlling Concurrency (max_data)
- `quiche/src/lib.rs:2134` — `max_tx_data` initialized to 0
- `quiche/src/lib.rs:7827` — `max_tx_data` set from transport params
