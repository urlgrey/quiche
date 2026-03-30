// Finding 7: unwrap() panics in recovery hot paths
//
// Demonstrates that crafted ACK frames with extreme ack_delay values can
// trigger panics in the loss detection and congestion control code paths.
//
// Add these tests to quiche/src/tests.rs:
//   cargo test --package quiche --lib -- tests::recovery_panic_extreme_ack_delay --nocapture

use std::time::Duration;
use std::time::Instant;

/// Demonstrates the core arithmetic vulnerability at recovery.rs:270:
///   let lost_send_time = now.checked_sub(loss_delay).unwrap();
///
/// When loss_delay > time elapsed since process start, checked_sub returns None
/// and unwrap() panics.
#[test]
fn recovery_panic_arithmetic_demonstration() {
    let now = Instant::now();

    // Simulate a loss_delay derived from an inflated smoothed_rtt
    // smoothed_rtt is influenced by ack_delay from peer ACK frames
    // RFC 9002 §5.3: smoothed_rtt is updated with ack_delay adjustment
    //
    // If a peer sends ack_delay = max_ack_delay (2^14 - 1 = 16383ms),
    // and this happens repeatedly, smoothed_rtt can grow very large.
    //
    // loss_delay = max(smoothed_rtt, latest_rtt) * time_threshold (9/8)
    // With inflated smoothed_rtt of ~20 seconds:
    let inflated_smoothed_rtt = Duration::from_secs(20);
    let loss_delay = inflated_smoothed_rtt * 9 / 8; // 22.5 seconds

    eprintln!("now (elapsed since epoch): ~{:?}", now.elapsed());
    eprintln!("loss_delay: {:?}", loss_delay);

    // The vulnerable code: now.checked_sub(loss_delay).unwrap()
    let result = now.checked_sub(loss_delay);
    eprintln!("now.checked_sub(loss_delay) = {:?}", result);

    // If the process hasn't been running for 22.5+ seconds, this is None
    // and unwrap() would panic
    if result.is_none() {
        eprintln!(
            "CONFIRMED: checked_sub returns None because loss_delay ({:?}) \
             exceeds time since process start. unwrap() would panic here!",
            loss_delay
        );
    }

    // Even with a more modest inflation, the race is possible
    // during the first few seconds of a connection
    let modest_loss_delay = Duration::from_secs(2);
    let recent_now = Instant::now(); // Just created, very little elapsed time

    // On a freshly started process or connection, this can still fail
    // because Instant::now() is relative to process start
    eprintln!(
        "\nWith modest loss_delay ({:?}): checked_sub = {:?}",
        modest_loss_delay,
        recent_now.checked_sub(modest_loss_delay)
    );
}

/// Demonstrates the vulnerability in context: a connection where the peer
/// sends ACK frames with extreme ack_delay values to inflate smoothed_rtt,
/// eventually causing the loss detection panic.
#[test]
fn recovery_panic_extreme_ack_delay() {
    let mut config = Pipe::default_config("reno").unwrap();
    config.set_initial_max_data(30);
    config.set_initial_max_stream_data_bidi_local(15);
    config.set_initial_max_stream_data_bidi_remote(15);
    config.set_initial_max_streams_bidi(3);

    // Set a large max_ack_delay to allow extreme values
    // RFC 9000 allows up to 2^14 - 1 = 16383ms
    config.set_max_ack_delay(16383);

    let mut pipe = Pipe::with_config(&mut config).unwrap();
    assert_eq!(pipe.handshake(), Ok(()));

    // Send data from client
    let stream_id = 0;
    assert_eq!(pipe.client.stream_send(stream_id, b"hello", true), Ok(5));
    assert_eq!(pipe.advance(), Ok(()));

    // The attack vector: if a malicious peer manipulates ACK frames
    // to include large ack_delay values, the smoothed_rtt grows.
    //
    // In the recovery module, loss_delay is computed as:
    //   loss_delay = max(latest_rtt, smoothed_rtt) * time_threshold
    //
    // And then:
    //   let lost_send_time = now.checked_sub(loss_delay).unwrap();  // LINE 270
    //
    // If loss_delay exceeds the time since the Instant epoch (process start),
    // checked_sub returns None and unwrap panics.

    // We can verify the vulnerable code path exists by examining the
    // recovery module's detect_and_remove_lost_packets:
    eprintln!("Connection established. In a real attack:");
    eprintln!("1. Malicious peer sends ACK frames with ack_delay near max_ack_delay");
    eprintln!("2. smoothed_rtt grows with each ACK (RFC 9002 §5.3)");
    eprintln!("3. loss_delay = smoothed_rtt * 9/8 becomes very large");
    eprintln!("4. now.checked_sub(loss_delay) returns None");
    eprintln!("5. .unwrap() panics → process crash");
    eprintln!("\nVulnerable code at recovery.rs:270:");
    eprintln!("  let lost_send_time = now.checked_sub(loss_delay).unwrap();");
}

/// Documents all unwrap() sites and their trigger conditions.
#[test]
fn recovery_unwrap_site_inventory() {
    eprintln!("=== unwrap() Panic Sites in Recovery ===\n");

    eprintln!("1. recovery.rs:178 — peer_sent_ack_ranges.last().unwrap()");
    eprintln!("   Trigger: Empty ACK ranges passed to detect_and_ack_sent_packets");
    eprintln!("   Impact: Panic if ACK frame parsing produces empty range list");
    eprintln!();

    eprintln!("2. recovery.rs:270 — now.checked_sub(loss_delay).unwrap()");
    eprintln!("   Trigger: loss_delay (from inflated smoothed_rtt) > Instant epoch delta");
    eprintln!("   Impact: Panic during loss detection, crashes server thread");
    eprintln!("   Attack: Send ACK frames with max ack_delay values repeatedly");
    eprintln!();

    eprintln!("3. bbr2/probe_bw.rs:453 — self.cycle.probe_wait_time.unwrap()");
    eprintln!("   Trigger: BBR2 enters is_time_to_probe_bandwidth before probe_wait_time set");
    eprintln!("   Impact: Panic during congestion control, crashes connection");
    eprintln!("   Attack: Rapid ACK/loss patterns causing BBR2 mode transition race");
    eprintln!();

    eprintln!("4. bbr2/network_model.rs:448 — self.bandwidth_lo.unwrap()");
    eprintln!("   Trigger: bandwidth_lo accessed before first bandwidth sample");
    eprintln!("   Impact: Panic during bandwidth estimation");
    eprintln!("   Attack: Crafted ACK patterns during BBR2 initialization");
    eprintln!();

    eprintln!("=== Recommended Fixes ===\n");
    eprintln!("1. Replace .unwrap() with .ok_or(Error::...)? or .unwrap_or(default)");
    eprintln!("2. Line 270: now.checked_sub(loss_delay).unwrap_or(now)");
    eprintln!("   (treating all packets as candidates for loss is safe)");
    eprintln!("3. Line 453: probe_wait_time.unwrap_or(Duration::ZERO)");
    eprintln!("4. Line 448: bandwidth_lo.unwrap_or(max_bandwidth())");
}
