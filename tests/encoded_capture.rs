//! Encoded (passthrough) capture: the receiver hands out the VMX1 bitstream it was
//! sent, so a consumer can store the frames without re-encoding them.
//!
//! Replay passthrough uses this. The capture is off until a consumer asks for it, and
//! what it replaces is counted, so a consumer that falls behind shows up as a gap
//! rather than as silence.

use openmediatransport::{Codec, FrameType, MediaFrame, ReceiverConfig, ReceiverSession, Sender};
use std::thread;
use std::time::Duration;
use vmx::{Codec as VmxCodec, Config as VmxConfig, Profile};

/// Encodes one colour-bar frame to a VMX1 bitstream, the way a sender would.
fn bitstream(width: i32, height: i32, shift: u8) -> Vec<u8> {
    let stride = (width as usize) * 2;
    let mut uyvy = vec![128u8; stride * height as usize];
    for y in 0..height as usize {
        for x in (0..width as usize).step_by(2) {
            let o = y * stride + x * 2;
            uyvy[o] = 128;
            uyvy[o + 1] = 16 + ((x + y + usize::from(shift)) % 220) as u8;
            uyvy[o + 2] = 128;
            uyvy[o + 3] = 16 + ((x + 1 + y + usize::from(shift)) % 220) as u8;
        }
    }
    let mut enc = VmxCodec::new(VmxConfig {
        width,
        height,
        profile: Profile::OmtLq,
        color_space: Default::default(),
    })
    .unwrap();
    enc.encode_uyvy(&uyvy, stride).unwrap();
    let mut out = vec![0u8; 2 << 20];
    let len = enc.save_to(&mut out).unwrap();
    out.truncate(len);
    out
}

/// Connects a receiver to a fresh sender and waits until the sender is subscribed.
fn pair() -> (Sender, ReceiverSession) {
    let mut sender = Sender::create("CaptureSrc", FrameType::VIDEO).expect("sender");
    let port = sender.port();
    let url = format!("omt://127.0.0.1:{port}");
    let session = ReceiverSession::connect(
        url,
        ReceiverConfig {
            frame_types: FrameType::VIDEO,
            connect_timeout: Duration::from_secs(2),
            ..ReceiverConfig::default()
        },
    )
    .expect("session");
    for _ in 0..100 {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.video_subscribed() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if !sender.video_subscribed() {
        sender.force_subscribe(true, false, false);
    }
    (sender, session)
}

fn send_vmx1(sender: &mut Sender, width: i32, height: i32, timestamp: i64, payload: Vec<u8>) {
    let frame = MediaFrame {
        frame_type: FrameType::VIDEO,
        timestamp,
        codec: Codec::Vmx1 as i32,
        width,
        height,
        frame_rate_n: 60_000,
        frame_rate_d: 1_001,
        aspect_ratio: 16.0 / 9.0,
        data: payload,
        ..Default::default()
    };
    sender.send_video(frame).expect("send");
}

#[test]
fn capture_hands_out_the_bitstream_it_was_sent() {
    let (mut sender, session) = pair();
    let width = 128i32;
    let height = 128i32;

    // Capture is off until asked for: decoding still works, and no bitstream is kept.
    let first = bitstream(width, height, 0);
    send_vmx1(&mut sender, width, height, 1_000_000, first);
    let mut decoded = None;
    for _ in 0..200 {
        if let Some(frame) = session.try_recv_video() {
            if frame.timestamp == 1_000_000 {
                decoded = Some(frame);
                break;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(decoded.is_some(), "the frame must still decode");
    assert!(
        session.try_recv_vmx1().is_none(),
        "nothing is captured before the consumer asks"
    );
    assert!(!session.encoded_capture());

    // With capture on, the bitstream arrives exactly as it was sent.
    session.set_encoded_capture(true);
    assert!(session.encoded_capture());
    let sent = bitstream(width, height, 7);
    send_vmx1(&mut sender, width, height, 2_000_000, sent.clone());
    let mut captured = None;
    for _ in 0..200 {
        if let Some(frame) = session.try_recv_vmx1() {
            captured = Some(frame);
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let frame = captured.expect("the captured bitstream");
    assert_eq!(frame.pts_100ns, 2_000_000);
    assert_eq!(frame.width, width as u32);
    assert_eq!(frame.height, height as u32);
    assert_eq!(frame.fps_num, 60_000);
    assert_eq!(frame.fps_den, 1_001);
    assert!(!frame.preview);
    assert_eq!(frame.sequence, 1, "the first captured frame is number one");
    assert_eq!(
        frame.payload, sent,
        "the captured bitstream is the one the sender sent, byte for byte"
    );

    // A consumer that does not take them loses the frames it missed, and they are
    // counted rather than silently dropped.
    for step in 0..4 {
        send_vmx1(
            &mut sender,
            width,
            height,
            3_000_000 + step * 100_000,
            bitstream(width, height, 20 + step as u8),
        );
        thread::sleep(Duration::from_millis(60));
    }
    let dropped = session.encoded_dropped();
    assert!(
        dropped >= 1,
        "frames the capture replaced must be counted, saw {dropped}"
    );
    let latest = session.try_recv_vmx1().expect("the latest bitstream");
    assert!(
        latest.sequence > frame.sequence,
        "the capture moves forward: {} then {}",
        frame.sequence,
        latest.sequence
    );
    assert!(latest.pts_100ns >= 3_000_000);

    // Turning it off drops what was waiting and stops copying payloads.
    session.set_encoded_capture(false);
    assert!(session.try_recv_vmx1().is_none());
    send_vmx1(
        &mut sender,
        width,
        height,
        9_000_000,
        bitstream(width, height, 40),
    );
    thread::sleep(Duration::from_millis(120));
    assert!(
        session.try_recv_vmx1().is_none(),
        "capture off keeps nothing"
    );
}
