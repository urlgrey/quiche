# Finding 9: GOAWAY + PRIORITY_UPDATE Race — Orphaned Stream State

## Severity
Medium

## Bug Class
State Machine / Memory Leak

## Affected Code
- File: `quiche/src/h3/mod.rs`
- Lines: 3140-3160 (PRIORITY_UPDATE handler `or_insert_with`)

## Description

The HTTP/3 PRIORITY_UPDATE frame handler creates stream entries for streams that have not yet been opened, using `self.streams.entry(id).or_insert_with(...)`. This is intentional — a client can send PRIORITY_UPDATE for a stream it intends to open later, and the server should store the priority information.

However, this mechanism does not check whether a GOAWAY frame has been sent for the connection. If the server has sent a GOAWAY with a stream ID lower than the PRIORITY_UPDATE's target stream, the server creates H3-layer stream state for a stream that **will never be opened** (since GOAWAY tells the client to stop opening new streams beyond that ID).

The relevant code at line 3140:

```rust
let stream = self.streams.entry(prioritized_element_id).or_insert_with(
    || <stream::Stream>::new(prioritized_element_id, false),
);
```

There is a check for collected streams (`is_collected`), but no check against the GOAWAY ID. A malicious client can send thousands of PRIORITY_UPDATE frames for stream IDs above the GOAWAY limit but below the `max_streams` limit. Each creates an H3 stream entry that will never be used and never cleaned up.

The only protection is the `max_streams` transport parameter, which limits the stream IDs that can be referenced. But with `max_streams_bidi` of even a few thousand, this allows creating a proportional number of orphaned entries.

## Reproduction Steps

```bash
cargo test --package quiche --lib h3::tests::goaway_priority_update_creates_orphaned_state --nocapture
```

Or conceptually:

1. Establish an HTTP/3 connection (client + server)
2. Exchange some requests normally
3. Server sends GOAWAY with stream ID 4 (only streams 0, 4 were/can be opened)
4. Client ignores GOAWAY and sends PRIORITY_UPDATE for stream IDs 8, 12, 16, 20...
5. Each PRIORITY_UPDATE creates an H3 stream entry on the server
6. These streams will never be opened (above GOAWAY ID) so the entries are never cleaned up
7. Memory grows linearly with the number of PRIORITY_UPDATE frames sent

## Expected vs Actual Behavior
- **Expected:** PRIORITY_UPDATE for stream IDs above the GOAWAY limit should be rejected or ignored, since those streams will never be opened.
- **Actual:** PRIORITY_UPDATE creates H3 stream entries for any valid (within max_streams) stream ID, regardless of GOAWAY. These entries become orphaned and are never garbage collected.

## Impact

A malicious client can cause unbounded memory growth on the server by:

1. Establishing an HTTP/3 connection
2. Triggering (or waiting for) a GOAWAY from the server
3. Sending thousands of PRIORITY_UPDATE frames for stream IDs above the GOAWAY limit
4. Each frame creates ~100-200 bytes of H3 stream state that is never freed
5. With `max_streams_bidi = 100`, this allows ~400 orphaned entries per connection (stream IDs 0..400 in steps of 4)
6. Across many connections, this becomes a significant memory leak

The fix should add a GOAWAY check in the PRIORITY_UPDATE handler:

```rust
// Reject PRIORITY_UPDATE for streams above GOAWAY limit
if let Some(goaway_id) = self.local_goaway_id {
    if prioritized_element_id >= goaway_id {
        return Err(Error::Done);
    }
}
```

## References
- RFC 9114 §5.2 — Connection Shutdown (GOAWAY)
- RFC 9218 §7 — PRIORITY_UPDATE Frame
- RFC 9114 §7.2.8 — GOAWAY frame processing rules
