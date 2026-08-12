//! Verify the bumped `vmx-rs` SIMD dispatch works through this crate.

use openmediatransport::vmx::{Codec, Config, Profile, SimdPath};
use std::thread;
use std::time::Duration;

use openmediatransport::{
    Codec as OmtCodec, FrameType, MediaFrame, ReceiverConfig, ReceiverSession, Sender,
};

fn make_uyvy(width: i32, height: i32) -> (Vec<u8>, usize) {
    let stride = (width as usize) * 2;
    let mut frame = vec![128u8; stride * height as usize];
    for y in 0..height as usize {
        for x in (0..width as usize).step_by(2) {
            let o = y * stride + x * 2;
            frame[o] = (100 + ((x / 2) % 40) as u8).min(240);
            frame[o + 1] = 16 + ((x + y * 3) % 220) as u8;
            frame[o + 2] = (140 + ((y / 4) % 40) as u8).min(240);
            frame[o + 3] = 16 + ((x + 1 + y * 5) % 220) as u8;
        }
    }
    (frame, stride)
}

#[test]
fn vmx_simd_path_is_reported_and_encodes() {
    // 1920 → UV width 960 (% 16 == 0) so AVX2 is eligible on capable hosts.
    let enc = Codec::new(Config {
        width: 1920,
        height: 1080,
        profile: Profile::OmtHq,
        color_space: Default::default(),
    })
    .expect("create codec");

    let path = enc.simd_path();
    let caps = enc.simd_capabilities();
    eprintln!(
        "vmx simd_path={path} caps={{ssse3:{},sse42:{},avx2:{},bmi2:{},neon:{}}}",
        caps.ssse3, caps.sse42, caps.avx2, caps.bmi2, caps.neon
    );

    assert!(
        matches!(
            path,
            SimdPath::Scalar | SimdPath::Sse128 | SimdPath::Avx2 | SimdPath::Neon
        ),
        "unexpected path {path}"
    );
    assert_eq!(path.to_string(), path.as_str());
    assert_eq!(caps.select_path(960), path);

    #[cfg(target_arch = "x86_64")]
    {
        assert_ne!(path, SimdPath::Neon);
        if caps.avx2 && caps.bmi2 {
            assert_eq!(path, SimdPath::Avx2);
        } else if caps.sse42 && caps.ssse3 {
            assert_eq!(path, SimdPath::Sse128);
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        assert!(matches!(path, SimdPath::Neon | SimdPath::Scalar));
    }

    // Geometry gate: UV width not multiple of 16 must not select AVX2.
    let odd = Codec::new(Config::new(632, 64)).expect("odd-width codec");
    assert_ne!(odd.simd_path(), SimdPath::Avx2);

    let (frame, stride) = make_uyvy(256, 144);
    let mut small = Codec::new(Config {
        width: 256,
        height: 144,
        profile: Profile::OmtLq,
        color_space: Default::default(),
    })
    .expect("small codec");
    let encode_path = small.simd_path();
    small.encode_uyvy(&frame, stride).expect("encode");
    let mut bitstream = vec![0u8; 2 << 20];
    let len = small.save_to(&mut bitstream).expect("save");
    assert!(len > 16, "bitstream too short: {len}");

    let mut dec = Codec::new(Config::new(256, 144)).expect("decoder");
    assert_eq!(dec.simd_path(), encode_path);
    dec.load_from(&bitstream[..len]).expect("load");
    let mut out = vec![0u8; stride * 144];
    dec.decode_uyvy(&mut out, stride).expect("decode");
    let mean: f32 = out.iter().map(|&b| b as f32).sum::<f32>() / out.len() as f32;
    assert!(mean > 1.0, "decoded frame looks empty (mean={mean})");
}

#[test]
fn vmx_simd_omt_loopback_uses_selected_path() {
    let mut sender = Sender::create("VmxSimdSrc", FrameType::VIDEO).expect("sender");
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

    let width = 320i32;
    let height = 180i32;
    let (uyvy, stride) = make_uyvy(width, height);
    let mut enc = Codec::new(Config {
        width,
        height,
        profile: Profile::OmtSq,
        color_space: Default::default(),
    })
    .unwrap();
    let path = enc.simd_path();
    eprintln!("OMT loopback encode path={path}");
    enc.encode_uyvy(&uyvy, stride).unwrap();
    let mut bitstream = vec![0u8; 2 << 20];
    let len = enc.save_to(&mut bitstream).unwrap();

    let frame = MediaFrame {
        frame_type: FrameType::VIDEO,
        timestamp: 42_000_000,
        codec: OmtCodec::Vmx1 as i32,
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
        if let Some(f) = session.try_recv_video()
            && f.timestamp == 42_000_000
        {
            got = Some(f);
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let frame = got.expect("decoded BGRA frame via OMT with updated vmx-rs");
    assert_eq!(frame.width, width as u32);
    assert_eq!(frame.height, height as u32);
    assert_eq!(frame.pixels.len(), (width * height * 4) as usize);
    assert!(
        frame.pixels.iter().any(|&b| b != 0),
        "BGRA looks empty on path {path}"
    );
    session.disconnect();
}
