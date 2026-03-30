// Finding 11: TxBufferTrackingState is observational only — no self-healing
//
// Demonstrates that once TxBufferTrackingState::Inconsistent is set,
// the connection never recovers. The state is permanent and tx_buffered
// is never corrected.
//
// Add to quiche/src/tests.rs:
//   cargo test --package quiche --lib -- tests::tx_buffered_inconsistent_state_is_permanent --nocapture

/// Once Inconsistent is set, it stays forever — no recovery mechanism exists.
#[test]
fn tx_buffered_inconsistent_state_is_permanent() {
    let mut config = Pipe::default_config("reno").unwrap();
    config.set_initial_max_data(1000);
    config.set_initial_max_stream_data_bidi_local(500);
    config.set_initial_max_stream_data_bidi_remote(500);
    config.set_initial_max_streams_bidi(5);

    let mut pipe = Pipe::with_config(&mut config).unwrap();
    assert_eq!(pipe.handshake(), Ok(()));

    // Verify initial state
    assert_eq!(pipe.client.tx_buffered_state, TxBufferTrackingState::Ok);
    assert_eq!(pipe.client.tx_buffered, 0);

    // Simulate the aftermath of a STOP_SENDING accounting bug:
    // tx_buffered is positive but no streams are flushable and no bytes in flight
    pipe.client.tx_buffered = 42;

    // The invariant check detects the problem
    pipe.client.check_tx_buffered_invariant();
    assert_eq!(
        pipe.client.tx_buffered_state,
        TxBufferTrackingState::Inconsistent,
        "State should be Inconsistent after detecting phantom tx_buffered"
    );

    // tx_buffered was NOT corrected
    assert_eq!(
        pipe.client.tx_buffered, 42,
        "tx_buffered should have been reset to 0 but wasn't"
    );

    // Now try to "heal" by doing normal operations:
    // Send data on a new stream, let it complete, drain everything
    let stream_id = 0;
    assert_eq!(pipe.client.stream_send(stream_id, b"test data", true), Ok(9));
    assert_eq!(pipe.advance(), Ok(()));

    // Server reads all data
    let mut buf = vec![0u8; 65535];
    loop {
        match pipe.server.stream_recv(stream_id, &mut buf) {
            Ok(_) => continue,
            Err(crate::Error::Done) => break,
            Err(_) => break,
        }
    }

    // Advance many times to fully settle
    for _ in 0..20 {
        let _ = pipe.advance();
    }

    // Check state again — still Inconsistent, no recovery
    assert_eq!(
        pipe.client.tx_buffered_state,
        TxBufferTrackingState::Inconsistent,
        "State remains Inconsistent even after successful data transfer"
    );

    eprintln!("=== State Permanence Demonstration ===");
    eprintln!("tx_buffered_state: {:?} (permanent)", pipe.client.tx_buffered_state);
    eprintln!("tx_buffered: {} (never corrected)", pipe.client.tx_buffered);
    eprintln!("The connection is permanently degraded with no recovery mechanism.");
}

/// Demonstrates the contrast: detection vs remediation.
/// Shows exactly what check_tx_buffered_invariant does and doesn't do.
#[test]
fn detection_without_remediation() {
    let mut config = Pipe::default_config("reno").unwrap();
    let mut pipe = Pipe::with_config(&mut config).unwrap();
    assert_eq!(pipe.handshake(), Ok(()));

    eprintln!("=== Detection vs Remediation ===\n");

    // 1. Set up the inconsistent state
    pipe.client.tx_buffered = 100;
    eprintln!("Before check:");
    eprintln!("  tx_buffered = {}", pipe.client.tx_buffered);
    eprintln!("  tx_buffered_state = {:?}", pipe.client.tx_buffered_state);
    eprintln!("  has_flushable = {}", pipe.client.streams.has_flushable());
    eprintln!("  bytes_in_flight = {}", pipe.client.paths.iter().map(|(_, p)| p.recovery.bytes_in_flight()).sum::<usize>());

    // 2. Run the invariant check
    pipe.client.check_tx_buffered_invariant();

    eprintln!("\nAfter check_tx_buffered_invariant():");
    eprintln!("  tx_buffered = {} (UNCHANGED — should be 0)", pipe.client.tx_buffered);
    eprintln!("  tx_buffered_state = {:?} (set to Inconsistent)", pipe.client.tx_buffered_state);

    // 3. Show impact on tx_cap
    pipe.client.update_tx_cap();
    eprintln!("\nAfter update_tx_cap():");
    eprintln!("  tx_cap = {} (reduced by phantom tx_buffered)", pipe.client.tx_cap);

    // 4. What SHOULD happen (the fix):
    eprintln!("\n=== What SHOULD happen ===");
    eprintln!("check_tx_buffered_invariant should:");
    eprintln!("  1. Set tx_buffered_state = Inconsistent ✓ (it does this)");
    eprintln!("  2. Reset tx_buffered = 0               ✗ (it does NOT do this)");
    eprintln!("  3. Log a warning                       ✗ (it does NOT do this)");
    eprintln!("  4. Emit a connection event              ✗ (it does NOT do this)");

    // Verify the bug
    assert_eq!(pipe.client.tx_buffered, 100, "tx_buffered was not corrected");
    assert_eq!(
        pipe.client.tx_buffered_state,
        TxBufferTrackingState::Inconsistent,
        "State was set but that's ALL that happened"
    );
}

/// Shows that the state transition is one-way: Ok → Inconsistent, never back.
#[test]
fn state_transition_is_one_way() {
    let mut config = Pipe::default_config("reno").unwrap();
    let mut pipe = Pipe::with_config(&mut config).unwrap();
    assert_eq!(pipe.handshake(), Ok(()));

    // Start Ok
    assert_eq!(pipe.client.tx_buffered_state, TxBufferTrackingState::Ok);

    // Trigger Inconsistent
    pipe.client.tx_buffered = 1;
    pipe.client.check_tx_buffered_invariant();
    assert_eq!(pipe.client.tx_buffered_state, TxBufferTrackingState::Inconsistent);

    // Manually fix tx_buffered (as if the accounting corrected itself)
    pipe.client.tx_buffered = 0;

    // Run the check again — it won't fire (tx_buffered == 0)
    // but it also won't reset the state back to Ok
    pipe.client.check_tx_buffered_invariant();

    assert_eq!(
        pipe.client.tx_buffered_state,
        TxBufferTrackingState::Inconsistent,
        "State never transitions back to Ok — it's a one-way flag"
    );

    eprintln!("Even after tx_buffered is manually fixed to 0,");
    eprintln!("tx_buffered_state remains {:?} forever.", pipe.client.tx_buffered_state);
    eprintln!("There is no path from Inconsistent back to Ok.");
}
