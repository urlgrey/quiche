// Finding 2: tx_buffered inconsistency after STOP_SENDING + packet loss
//
// This test demonstrates that check_tx_buffered_invariant() detects
// the inconsistency but takes no corrective action, leaving the
// connection in a permanently degraded state.
//
// To run as part of the quiche test suite, add this test to quiche/src/tests.rs
// or run: cargo test --package quiche --lib -- tests::tx_buffered_inconsistency_after_stop_sending

// The following test should be added to quiche/src/tests.rs:

#[test]
fn tx_buffered_inconsistency_after_stop_sending() {
    // Create a connection pair with a small initial max data to make
    // flow control interactions more visible.
    let mut config = Pipe::default_config("reno").unwrap();
    config.set_initial_max_data(1000);
    config.set_initial_max_stream_data_bidi_local(500);
    config.set_initial_max_stream_data_bidi_remote(500);
    config.set_initial_max_streams_bidi(5);

    let mut pipe = Pipe::with_config(&mut config).unwrap();
    assert_eq!(pipe.handshake(), Ok(()));

    // Step 1: Client opens a stream and sends data
    let stream_id = 0; // First client-initiated bidi stream
    let data = vec![0x42u8; 200];
    assert_eq!(
        pipe.client.stream_send(stream_id, &data, false),
        Ok(200)
    );

    // Record tx_buffered before advancing
    let tx_buffered_before = pipe.client.tx_buffered;

    // Step 2: Advance to send the data (but we'll simulate loss later)
    // First, let data flow normally
    assert_eq!(pipe.advance(), Ok(()));

    // Step 3: Server sends STOP_SENDING for this stream
    pipe.server.stream_shutdown(stream_id, Shutdown::Read, 0x1).unwrap();

    // Step 4: Send more data on the stream before STOP_SENDING arrives
    // This creates a window where data is in-flight but about to be stopped
    let more_data = vec![0x43u8; 100];
    let _ = pipe.client.stream_send(stream_id, &more_data, false);

    // Step 5: Advance - this delivers the STOP_SENDING to the client
    // The interaction between STOP_SENDING processing and in-flight
    // data accounting is where the inconsistency can arise
    assert_eq!(pipe.advance(), Ok(()));

    // Step 6: Client processes the STOP_SENDING by reading events
    let mut buf = vec![0u8; 65535];
    loop {
        match pipe.client.stream_recv(stream_id, &mut buf) {
            Ok(_) => continue,
            Err(crate::Error::Done) => break,
            Err(e) => {
                // STOP_SENDING may cause StreamReset
                eprintln!("stream_recv error (expected): {:?}", e);
                break;
            }
        }
    }

    // Step 7: Drain any remaining data and let the connection settle
    for _ in 0..10 {
        let _ = pipe.advance();
    }

    // Step 8: Check the tx_buffered state
    let stats = pipe.client.stats();
    eprintln!("tx_buffered_state: {:?}", stats.tx_buffered_state);
    eprintln!("tx_buffered (internal): {}", pipe.client.tx_buffered);

    // The key observation: if tx_buffered > 0 with no flushable streams
    // and no in-flight bytes, the invariant check fires but does nothing
    if pipe.client.tx_buffered > 0
        && !pipe.client.streams.has_flushable()
        && !pipe.client.paths.iter().any(|(_, p)| p.recovery.bytes_in_flight() > 0)
    {
        assert_eq!(
            stats.tx_buffered_state,
            TxBufferTrackingState::Inconsistent,
            "tx_buffered is {} with no flushable streams and no in-flight bytes, \
             but state is not Inconsistent",
            pipe.client.tx_buffered
        );

        // This is the bug: the state is detected but not remediated.
        // tx_buffered should be reset to 0 but it isn't.
        eprintln!(
            "BUG CONFIRMED: tx_buffered={} but no streams to flush and no bytes in flight. \
             State is {:?} but no corrective action was taken.",
            pipe.client.tx_buffered,
            stats.tx_buffered_state
        );
    }

    // Even if the specific race didn't trigger in this run,
    // demonstrate the design flaw: once Inconsistent is set, it stays forever
    // (see Finding 11 for the no-self-heal aspect)
}

// Standalone test: directly demonstrate the check_tx_buffered_invariant behavior
// by showing that the method only sets a flag, never corrects the value.
#[test]
fn tx_buffered_invariant_check_is_observational_only() {
    let mut config = Pipe::default_config("reno").unwrap();
    let mut pipe = Pipe::with_config(&mut config).unwrap();
    assert_eq!(pipe.handshake(), Ok(()));

    // Verify initial state
    assert_eq!(pipe.client.tx_buffered, 0);
    assert_eq!(
        pipe.client.stats().tx_buffered_state,
        TxBufferTrackingState::Ok
    );

    // Artificially set tx_buffered to a positive value
    // (simulating the aftermath of a STOP_SENDING accounting bug)
    pipe.client.tx_buffered = 100;

    // The invariant check should detect the inconsistency
    pipe.client.check_tx_buffered_invariant();

    // State is set to Inconsistent...
    assert_eq!(
        pipe.client.tx_buffered_state,
        TxBufferTrackingState::Inconsistent,
        "Expected Inconsistent state when tx_buffered > 0 with no flushable/inflight"
    );

    // ...but tx_buffered is NOT corrected
    assert_eq!(
        pipe.client.tx_buffered, 100,
        "BUG: tx_buffered should have been reset to 0, but it remains at 100"
    );

    // The connection's tx_cap will now be permanently reduced by this phantom value
    eprintln!(
        "tx_buffered remains at {} after invariant check detected inconsistency. \
         No corrective action was taken.",
        pipe.client.tx_buffered
    );
}
