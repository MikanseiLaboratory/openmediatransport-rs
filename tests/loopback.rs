//! Loopback send/receive integration test.

use openmediatransport::{
    FrameType, MediaFrame, ReceiverConfig, ReceiverSession, Sender, Settings,
    protocol::metadata::SUBSCRIBE_VIDEO,
};
use std::thread;
use std::time::Duration;

#[test]
fn metadata_subscribe_and_video_roundtrip() {
    let mut sender =
        Sender::create("TestSrc", FrameType::VIDEO | FrameType::METADATA).expect("sender");
    let port = sender.port();
    let (start, end) = Settings::global()
        .lock()
        .expect("settings lock")
        .network_port_range();
    assert!(
        (start..=end).contains(&port),
        "sender port {port} outside configured range {start}..={end}"
    );

    let url = format!("omt://127.0.0.1:{port}");
    let session = ReceiverSession::connect(
        &url,
        ReceiverConfig {
            frame_types: FrameType::VIDEO | FrameType::METADATA,
            ..ReceiverConfig::default()
        },
    )
    .expect("rx");

    for _ in 0..100 {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.video_subscribed() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    if !sender.video_subscribed() {
        sender.force_subscribe(true, false, true);
    }
    assert!(sender.video_subscribed());

    let width = 16i32;
    let height = 16i32;
    let stride = (width as usize) * 2;
    let payload = vec![128u8; stride * height as usize];
    let frame = MediaFrame {
        frame_type: FrameType::VIDEO,
        timestamp: 12345,
        codec: openmediatransport::Codec::Uyvy as i32,
        width,
        height,
        stride: stride as i32,
        frame_rate_n: 60,
        frame_rate_d: 1,
        aspect_ratio: 1.0,
        data: payload,
        ..Default::default()
    };
    sender.send_video(frame).expect("send");

    let mut got = None;
    for _ in 0..200 {
        if let Some(f) = session.try_recv_video()
            && f.timestamp == 12345
        {
            got = Some(f);
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let frame = got.expect("decoded uncompressed UYVY");
    assert_eq!(frame.width, width as u32);
    assert_eq!(frame.height, height as u32);
    assert_eq!(frame.pixels.len(), (width * height * 4) as usize);
    let _ = SUBSCRIBE_VIDEO;
    session.disconnect();
}

#[test]
fn vmx_colorbar_loopback_decodes_bgra() {
    use openmediatransport::Codec;
    use vmx::{Codec as VmxCodec, Config as VmxConfig, Profile};

    let mut sender = Sender::create("VmxSrc", FrameType::VIDEO).expect("sender");
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

    let width = 128i32;
    let height = 128i32;
    let stride = (width as usize) * 2;
    let mut uyvy = vec![128u8; stride * height as usize];
    for y in 0..height as usize {
        for x in (0..width as usize).step_by(2) {
            let o = y * stride + x * 2;
            uyvy[o] = 128;
            uyvy[o + 1] = 16 + ((x + y) % 220) as u8;
            uyvy[o + 2] = 128;
            uyvy[o + 3] = 16 + ((x + 1 + y) % 220) as u8;
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
    let mut bitstream = vec![0u8; 2 << 20];
    let len = enc.save_to(&mut bitstream).unwrap();

    let frame = MediaFrame {
        frame_type: FrameType::VIDEO,
        timestamp: 10_000_000,
        codec: Codec::Vmx1 as i32,
        width,
        height,
        frame_rate_n: 60_000,
        frame_rate_d: 1_001,
        aspect_ratio: 16.0 / 9.0,
        data: bitstream[..len].to_vec(),
        ..Default::default()
    };
    sender.send_video(frame).unwrap();

    let mut got = None;
    for _ in 0..200 {
        if let Some(f) = session.try_recv_video() {
            // Parallel tests can briefly race; wait for our own timestamp.
            if f.timestamp == 10_000_000 {
                got = Some(f);
                break;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    let frame = got.expect("decoded BGRA frame");
    assert_eq!(frame.timestamp, 10_000_000);
    assert_eq!(frame.width, width as u32);
    assert_eq!(frame.height, height as u32);
    assert_eq!(frame.pixels.len(), (width * height * 4) as usize);
    session.disconnect();
}

#[test]
fn vmx_preview_loopback_decodes_eighth_bgra() {
    use openmediatransport::Codec;
    use vmx::{Codec as VmxCodec, Config as VmxConfig, Profile};

    let mut sender = Sender::create("VmxPreviewSrc", FrameType::VIDEO).expect("sender");
    let port = sender.port();
    let url = format!("omt://127.0.0.1:{port}");
    let session = ReceiverSession::connect(
        url,
        ReceiverConfig {
            frame_types: FrameType::VIDEO,
            preview: true,
            connect_timeout: Duration::from_secs(2),
            ..ReceiverConfig::default()
        },
    )
    .expect("session");

    for _ in 0..100 {
        let _ = sender.poll_accept();
        let _ = sender.poll_peer_metadata();
        if sender.video_subscribed() && sender.preview() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if !sender.video_subscribed() {
        sender.force_subscribe(true, false, false);
    }
    if !sender.preview() {
        sender.force_preview(true);
    }
    assert!(sender.preview());

    let width = 128i32;
    let height = 128i32;
    let stride = (width as usize) * 2;
    let mut uyvy = vec![128u8; stride * height as usize];
    for y in 0..height as usize {
        for x in (0..width as usize).step_by(2) {
            let o = y * stride + x * 2;
            uyvy[o] = 128;
            uyvy[o + 1] = 16 + ((x + y) % 220) as u8;
            uyvy[o + 2] = 128;
            uyvy[o + 3] = 16 + ((x + 1 + y) % 220) as u8;
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
    let mut bitstream = vec![0u8; 2 << 20];
    let full_len = enc.save_to(&mut bitstream).unwrap();
    let preview_len = enc.get_encoded_preview_length();
    assert!(preview_len < full_len);

    let frame = MediaFrame {
        frame_type: FrameType::VIDEO,
        timestamp: 20_000_000,
        codec: Codec::Vmx1 as i32,
        width,
        height,
        frame_rate_n: 60_000,
        frame_rate_d: 1_001,
        aspect_ratio: 16.0 / 9.0,
        data: bitstream[..full_len].to_vec(),
        ..Default::default()
    };
    sender.send_video(frame).unwrap();

    let mut got = None;
    for _ in 0..200 {
        if let Some(f) = session.try_recv_video() {
            // Parallel tests can briefly race; wait for our own timestamp.
            if f.timestamp == 20_000_000 {
                got = Some(f);
                break;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    let frame = got.expect("decoded preview BGRA frame");
    assert_eq!(frame.timestamp, 20_000_000);
    assert_eq!(frame.width, 16);
    assert_eq!(frame.height, 16);
    assert_eq!(frame.pixels.len(), 16 * 16 * 4);
    session.disconnect();
}
