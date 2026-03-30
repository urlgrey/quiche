# Finding 7: `unwrap()` Panics in Recovery Hot Paths

## Severity
Medium

## Bug Class
Panic / Denial of Service

## Affected Code
- File: `quiche/src/recovery/gcongestion/recovery.rs`
  - Line 178: `peer_sent_ack_ranges.last().unwrap()` — panics if ACK ranges are empty
  - Line 270: `now.checked_sub(loss_delay).unwrap()` — panics if `loss_delay > now.elapsed()`
- File: `quiche/src/recovery/gcongestion/bbr2/probe_bw.rs`
  - Line 453: `self.cycle.probe_wait_time.unwrap()` — panics if `probe_wait_time` is None (uninitialized during mode transitions)
- File: `quiche/src/recovery/gcongestion/bbr2/network_model.rs`
  - Line 448: `self.bandwidth_lo.unwrap()` — panics if `bandwidth_lo` is None

## Description

Several `unwrap()` calls in the congestion control and loss recovery code paths will panic if the underlying `Option` is `None` or if a checked arithmetic operation fails. These are reachable from the ACK processing hot path: `Connection::recv()` → ACK frame handling → loss detection → congestion control updates.

The most critical site is at `recovery.rs:270`:

```rust
let lost_send_time = now.checked_sub(loss_delay).unwrap();
```

Here, `loss_delay` is derived from `smoothed_rtt` (which is updated from ACK frame timestamps). If a peer sends crafted ACK frames with extreme `ack_delay` values, the smoothed RTT can grow to a value that exceeds the duration since `now` was captured. When `checked_sub` returns `None` (because the subtraction would underflow `Instant`), the `unwrap()` panics, crashing the QUIC server.

The BBR2 sites (`probe_bw.rs:453`, `network_model.rs:448`) can be triggered by state machine race conditions during congestion control mode transitions. If the BBR2 state machine enters `is_time_to_probe_bandwidth` before `probe_wait_time` has been initialized (e.g., during rapid mode switching caused by crafted ACK patterns), the `unwrap()` panics.

At `recovery.rs:178`, `peer_sent_ack_ranges.last().unwrap()` assumes the ACK range list is non-empty. While the QUIC frame parser normally ensures this, edge cases in frame processing could potentially pass an empty range.

## Reproduction Steps

### Method 1: Crafted ACK delay (recovery.rs:270)

```bash
cargo test --package quiche --lib -- tests::recovery_panic_extreme_ack_delay --nocapture
```

1. Establish a QUIC connection
2. Client sends data packets
3. Server crafts ACK frames with manipulated `ack_delay` values
4. The inflated `ack_delay` causes `smoothed_rtt` to grow extremely large
5. `loss_delay` (derived from smoothed_rtt) exceeds `now - time_sent`
6. `now.checked_sub(loss_delay)` returns `None`
7. `.unwrap()` panics → server crash

### Method 2: BBR2 uninitialized state (probe_bw.rs:453)

1. Establish a QUIC connection using BBR2 congestion control
2. Send rapid bursts of data to trigger frequent congestion events
3. Send crafted ACK patterns that cause rapid BBR2 mode transitions
4. If the state machine enters `is_time_to_probe_bandwidth` before `probe_wait_time` is set, the `unwrap()` panics

### Running the repro tests:

```bash
cd /path/to/quiche
cargo test --package quiche --lib -- tests::recovery_panic_extreme_ack_delay --nocapture
```

## Expected vs Actual Behavior
- **Expected:** The recovery code handles edge cases gracefully — `checked_sub` failures should default to a safe value (e.g., treating all packets as potentially lost), and `Option` values should be checked before use.
- **Actual:** `unwrap()` causes a panic, crashing the entire QUIC server process. A single malicious client can take down a server handling thousands of connections.

## Impact

A remote attacker can crash a QUIC server by:

1. **Single-packet DoS:** Sending crafted ACK frames with extreme `ack_delay` values to trigger the `checked_sub().unwrap()` panic at line 270. This requires only a few packets after the handshake completes.
2. **BBR2 state confusion:** If the server uses BBR2 congestion control, crafted ACK patterns can trigger mode transition races that hit the `probe_wait_time.unwrap()` panic.
3. **Process-wide crash:** Since Rust panics unwind (or abort) the thread, and QUIC servers typically handle multiple connections per thread, one malicious client can disrupt all connections on that thread.

The fix is to replace `unwrap()` with safe alternatives:
- Line 270: `now.checked_sub(loss_delay).unwrap_or(now)` — treat all packets as potentially lost if the time math is invalid
- Line 178: Return an error if `peer_sent_ack_ranges` is empty
- Line 453: Use `unwrap_or` with a default probe wait time
- Line 448: Use `unwrap_or` with `max_bandwidth()`

## References
- RFC 9002 §6.1.2 — Time Threshold for loss detection
- RFC 9002 §5.3 — Estimating smoothed_rtt from ack_delay
- RFC 9000 §19.3 — ACK Frame format (ack_delay field)
- BBR v2 draft — Probe Bandwidth state machine
