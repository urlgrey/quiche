# Finding 11: `TxBufferTrackingState` is Observational Only — No Self-Healing

## Severity
Low

## Bug Class
Design Weakness / State Machine

## Affected Code
- File: `quiche/src/lib.rs`
- Lines: 1269-1278 (`TxBufferTrackingState` enum)
- Lines: 8846-8859 (`check_tx_buffered_invariant()`)

## Description

The `TxBufferTrackingState` enum was introduced as a diagnostic tool after the STOP_SENDING `tx_buffered` accounting bug (commit `aea7592`). It has two variants:

```rust
pub enum TxBufferTrackingState {
    #[default]
    Ok,
    Inconsistent,
}
```

The `check_tx_buffered_invariant()` method sets the state to `Inconsistent` when `tx_buffered > 0` with no flushable streams and no in-flight bytes. However, this is the extent of its functionality — it **detects** but does not **remediate**.

The key design issues:

1. **No corrective action:** When `Inconsistent` is detected, `tx_buffered` is not reset to 0. The connection remains permanently degraded.
2. **No recovery path:** There is no mechanism to transition back from `Inconsistent` to `Ok`. Once the flag is set, it stays set forever.
3. **Exposure only via stats:** The state is only accessible through `connection.stats().tx_buffered_state`. The application must actively poll this value and take action (e.g., close and re-establish the connection).
4. **No logging or alerting:** The state change is silent — no log message, no error, no event.

This is exactly the pattern that caused the original bug: the accounting invariant is violated, and the connection silently degrades. The detection was added post-fix but the recovery path wasn't, leaving a gap between diagnosis and treatment.

## Reproduction Steps

```bash
cargo test --package quiche --lib -- tests::tx_buffered_inconsistent_state_is_permanent --nocapture
```

1. Establish a QUIC connection
2. Artificially set `tx_buffered` to a positive value (simulating a STOP_SENDING accounting race)
3. Call `check_tx_buffered_invariant()` — state becomes `Inconsistent`
4. Continue sending data, process ACKs, let the connection run normally
5. Check the state again — it remains `Inconsistent` forever
6. Verify that `tx_buffered` was never corrected

## Expected vs Actual Behavior
- **Expected:** When `TxBufferTrackingState::Inconsistent` is detected, the connection should either (a) reset `tx_buffered` to 0 and transition back to `Ok`, or (b) emit a connection error/event so the application can take action, or (c) at minimum log a warning.
- **Actual:** The state is set to `Inconsistent` and nothing else happens. `tx_buffered` retains its phantom value, permanently reducing `tx_cap`. The connection silently degrades with no recovery mechanism.

## Impact

This finding amplifies Finding 2 (the `tx_buffered` inconsistency). When the accounting bug triggers:

1. `tx_buffered` becomes positive with no actual buffered data
2. `check_tx_buffered_invariant()` detects this and sets `Inconsistent`
3. **No corrective action is taken** — `tx_buffered` stays positive
4. `update_tx_cap()` uses `tx_buffered` to reduce `tx_cap`, permanently throttling the connection
5. The application would need to actively poll `stats().tx_buffered_state` and close/re-open the connection — but most applications don't know to do this
6. The connection is effectively stalled until closed

The contrast between **detection** and **remediation** is the core issue. The code knows something is wrong but does nothing about it, leaving the application to suffer silently.

A proper fix would be:

```rust
fn check_tx_buffered_invariant(&mut self) {
    if self.tx_buffered > 0
        && !self.streams.has_flushable()
        && !self.paths.iter().any(|(_, p)| p.recovery.bytes_in_flight() > 0)
    {
        warn!("tx_buffered invariant violated: {} bytes with no flushable/inflight", self.tx_buffered);
        self.tx_buffered = 0;  // Corrective action
        self.tx_buffered_state = TxBufferTrackingState::Inconsistent;
    }
}
```

## References
- Commit `aea7592` — original STOP_SENDING `tx_buffered` accounting fix
- Finding 2 — `tx_buffered` inconsistency detection without remediation
- `quiche/src/lib.rs:8846-8859` — `check_tx_buffered_invariant()` implementation
