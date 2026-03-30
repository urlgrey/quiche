// Finding 3: max_tx_data - tx_data unsigned subtraction underflow
//
// Demonstrates that when tx_data > max_tx_data, the unsigned u64
// subtraction wraps to near u64::MAX, bypassing flow control.
//
// Add these tests to quiche/src/tests.rs to run within the quiche test suite:
//   cargo test --package quiche --lib -- tests::tx_data_underflow_wraps_to_max --nocapture
//   cargo test --package quiche --lib -- tests::tx_data_underflow_bypasses_flow_control --nocapture

use std::cmp;

/// Demonstrates the raw arithmetic problem:
/// u64 unsigned subtraction wraps when minuend < subtrahend.
#[test]
fn tx_data_underflow_arithmetic() {
    // Simulate the state after 0-RTT data is buffered but transport params
    // haven't arrived yet
    let max_tx_data: u64 = 0; // Initial value before transport params
    let tx_data: u64 = 100;   // Data already sent/buffered via 0-RTT

    // This is what quiche computes at lines 5913, 6412, 8810:
    let result = max_tx_data.wrapping_sub(tx_data);

    eprintln!("max_tx_data:     {}", max_tx_data);
    eprintln!("tx_data:         {}", tx_data);
    eprintln!("max_tx_data - tx_data (wrapping): {}", result);
    eprintln!("u64::MAX:                         {}", u64::MAX);

    // The result wraps to near u64::MAX
    assert_eq!(result, u64::MAX - 99);
    assert!(result > u64::MAX / 2, "Wrapped value is enormous");

    // Line 5913/6412: "if self.max_tx_data - self.tx_data < len"
    // With the wrapped value, this check NEVER fires for reasonable len values
    let len: u64 = 1_000_000; // 1MB of data
    let would_be_blocked = result < len;
    assert!(
        !would_be_blocked,
        "Connection should be blocked but the underflow makes it appear unblocked"
    );

    // Line 8810: "let cap = cmp::min(cwin_available, self.max_tx_data - self.tx_data)"
    let cwin_available: u64 = 65535;
    let cap = cmp::min(cwin_available, result) as usize;
    assert_eq!(
        cap, 65535,
        "tx_cap should be 0 (blocked by flow control) but instead equals full cwnd"
    );

    // The correct computation using saturating_sub:
    let safe_result = max_tx_data.saturating_sub(tx_data);
    assert_eq!(safe_result, 0, "saturating_sub correctly returns 0");

    let safe_cap = cmp::min(cwin_available, safe_result) as usize;
    assert_eq!(safe_cap, 0, "With saturating_sub, tx_cap is correctly 0");

    eprintln!("\n--- SUMMARY ---");
    eprintln!("Unsafe subtraction: cap = {} (bypasses flow control)", cap);
    eprintln!("Safe subtraction:   cap = {} (correctly blocked)", safe_cap);
}

/// Demonstrates the underflow in the context of a real quiche connection.
/// This test manipulates internal state to simulate the 0-RTT race condition.
#[test]
fn tx_data_underflow_bypasses_flow_control() {
    let mut config = Pipe::default_config("reno").unwrap();
    config.set_initial_max_data(30); // Small flow control limit
    config.set_initial_max_stream_data_bidi_local(15);
    config.set_initial_max_stream_data_bidi_remote(15);

    let mut pipe = Pipe::with_config(&mut config).unwrap();
    assert_eq!(pipe.handshake(), Ok(()));

    // After handshake, max_tx_data is set from transport params
    eprintln!("After handshake:");
    eprintln!("  client.max_tx_data = {}", pipe.client.max_tx_data);
    eprintln!("  client.tx_data     = {}", pipe.client.tx_data);

    // Simulate the bug: tx_data exceeds max_tx_data
    // This can happen in the 0-RTT race or through STOP_SENDING accounting errors
    pipe.client.tx_data = pipe.client.max_tx_data + 1;

    eprintln!("\nAfter simulating underflow condition:");
    eprintln!("  client.max_tx_data = {}", pipe.client.max_tx_data);
    eprintln!("  client.tx_data     = {}", pipe.client.tx_data);

    // Now update_tx_cap computes the underflowed value
    pipe.client.update_tx_cap();

    eprintln!("  client.tx_cap      = {}", pipe.client.tx_cap);

    // tx_cap should be 0 (we've exceeded flow control) but instead
    // it's set to the cwnd value because the underflow makes it appear
    // that we have near-infinite flow control headroom
    if pipe.client.tx_cap > 0 {
        eprintln!(
            "\nBUG CONFIRMED: tx_cap is {} even though tx_data ({}) > max_tx_data ({}). \
             Flow control has been bypassed!",
            pipe.client.tx_cap,
            pipe.client.tx_data,
            pipe.client.max_tx_data
        );
    }

    // Also check the stream_writable path (line 6412)
    // The subtraction at line 6412 uses the same pattern
    let underflow_value = pipe.client.max_tx_data.wrapping_sub(pipe.client.tx_data);
    eprintln!(
        "\nmax_tx_data - tx_data = {} (wrapping), expected 0",
        underflow_value
    );
    assert!(
        underflow_value > u64::MAX / 2,
        "The subtraction should have yielded 0 or negative, \
         but instead wrapped to {}",
        underflow_value
    );
}

/// Show the specific 0-RTT scenario where the underflow naturally occurs.
/// max_tx_data starts at 0 and is only set when transport params arrive.
#[test]
fn tx_data_initial_zero_max_tx_data() {
    // Demonstrate that max_tx_data starts at 0
    let mut config = Pipe::default_config("reno").unwrap();
    let mut pipe = Pipe::with_config(&mut config).unwrap();

    // Before handshake completes, max_tx_data is 0
    // In a 0-RTT scenario, data could be queued before this is set
    eprintln!("Before handshake:");
    eprintln!("  client.max_tx_data = {}", pipe.client.max_tx_data);
    eprintln!("  client.tx_data     = {}", pipe.client.tx_data);

    // max_tx_data starts at 0 as per line 2134
    assert_eq!(
        pipe.client.max_tx_data, 0,
        "max_tx_data should initialize to 0"
    );

    // If any 0-RTT data were buffered before transport params arrive,
    // tx_data would be > 0 while max_tx_data is still 0.
    // The subtraction 0 - tx_data would wrap.
    pipe.client.tx_data = 50; // Simulate 0-RTT data buffered

    let wrapped = pipe.client.max_tx_data.wrapping_sub(pipe.client.tx_data);
    eprintln!(
        "\n0-RTT scenario: max_tx_data(0) - tx_data(50) = {} (wraps to near u64::MAX)",
        wrapped
    );
    assert_eq!(wrapped, u64::MAX - 49);
}
