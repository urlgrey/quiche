# Finding 2: `tx_buffered` Inconsistency Detection Without Remediation

## Severity
High

## Bug Class
Accounting / Denial of Service

## Affected Code
- File: `quiche/src/lib.rs`
- Lines: 8846-8859 (`check_tx_buffered_invariant()`)
- Related: `saturating_sub` sites at lines 3629, 4183, 6203, 6206, 8346, 8349

## Description

Quiche tracks buffered transmission data via `tx_buffered` and has a `TxBufferTrackingState` enum that detects when this counter becomes inconsistent — specifically when `tx_buffered > 0` but there are no flushable streams and no in-flight bytes.

The `check_tx_buffered_invariant()` method at line 8846 only **sets a flag** (`TxBufferTrackingState::Inconsistent`) but never resets `tx_buffered` to 0 or takes any corrective action. The flag is only exposed via `connection.stats().tx_buffered_state`.

This means if any code path fails to properly decrement `tx_buffered` (and there are several `saturating_sub` sites that could mask real underflows), the connection will **silently stall** — `tx_cap` will report capacity based on a phantom buffer that doesn't exist, throttling the connection below its actual capacity. The existence of `TxBufferTrackingState` itself is an admission that this invariant has been violated in production (see commit `aea7592`).

## Reproduction Steps

1. Create a QUIC connection using `test_utils::Pipe`
2. Send data on a stream via the client
3. Have the server issue STOP_SENDING on that stream
4. Simulate packet loss during the STOP_SENDING + retransmission sequence
5. After ACKs settle, check `connection.stats().tx_buffered_state`
6. Observe `TxBufferTrackingState::Inconsistent` — `tx_buffered` is positive with no data to send

Run the reproduction test:

```bash
cd /path/to/quiche
cargo test --package quiche --lib -- tests::tx_buffered_inconsistency_after_stop_sending --nocapture
```

Or run the standalone repro:

```bash
cd repro/
cargo test --nocapture
```

## Expected vs Actual Behavior
- **Expected:** After STOP_SENDING + retransmission, `tx_buffered` returns to 0 and the connection remains fully functional with `TxBufferTrackingState::Ok`.
- **Actual:** `tx_buffered` remains positive with no flushable streams or in-flight data. `TxBufferTrackingState::Inconsistent` is set but no corrective action is taken. The connection's `tx_cap` is permanently reduced by the phantom buffered bytes.

## Impact

An attacker can trigger a permanent connection stall by:
1. Opening streams and sending data
2. Issuing STOP_SENDING while data is in-flight
3. Causing packet loss (natural or induced) during the retransmission sequence

The connection becomes permanently throttled — `tx_cap` reports reduced capacity due to phantom `tx_buffered` bytes that don't correspond to any actual buffered data. The application has no way to recover short of closing and re-establishing the connection.

In a CDN or proxy deployment, this can be used to degrade connection performance for targeted clients.

## References
- Commit `aea7592` — original STOP_SENDING `tx_buffered` accounting fix
- RFC 9000 §3.5 — Solicited State Transitions (STOP_SENDING)
- `quiche/src/lib.rs:1269-1278` — `TxBufferTrackingState` enum definition
