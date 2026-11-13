#[path = "fixtures/support.rs"]
mod support;
use pin_assets::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::time::Instant;
use support::*;
static CUR: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
struct A;
unsafe impl GlobalAlloc for A {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        let c = CUR.fetch_add(l.size(), SeqCst) + l.size();
        PEAK.fetch_max(c, SeqCst);
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        let c = CUR.fetch_add(l.size(), SeqCst) + l.size();
        PEAK.fetch_max(c, SeqCst);
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        CUR.fetch_sub(l.size(), SeqCst);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if n > l.size() {
            let c = CUR.fetch_add(n - l.size(), SeqCst) + n - l.size();
            PEAK.fetch_max(c, SeqCst);
        } else {
            CUR.fetch_sub(l.size() - n, SeqCst);
        }
        q
    }
}
#[global_allocator]
static G: A = A;
fn reset() -> usize {
    let c = CUR.load(SeqCst);
    PEAK.store(c, SeqCst);
    c
}
fn extra(base: usize) -> f64 {
    (PEAK.load(SeqCst) - base) as f64 / 1048576.0
}

fn which() -> String {
    std::env::var("PROBE").unwrap_or_default()
}

#[test]
fn probe() {
    let w = which();
    // Build inputs first (not counted in timing; memory reported separately via baseline probe).
    match w.as_str() {
        "na" => {
            // 2M real central records (stored, empty names "a<i>"), Zip64 count, small 16-bit count.
            let n = 2_000_000u64;
            let mut out = Vec::new();
            let mut central = Vec::new();
            for i in 0..n {
                let name = format!("{i:07}");
                let off = out.len() as u32;
                out.extend(0x0403_4b50u32.to_le_bytes());
                out.extend([20, 0, 0, 8, 0, 0, 0, 0, 0, 0]);
                out.extend(0u32.to_le_bytes());
                out.extend(0u32.to_le_bytes());
                out.extend(0u32.to_le_bytes());
                out.extend((name.len() as u16).to_le_bytes());
                out.extend(0u16.to_le_bytes());
                out.extend(name.as_bytes());
                central.extend(0x0201_4b50u32.to_le_bytes());
                central.extend([20, 3, 20, 0, 0, 8, 0, 0, 0, 0, 0, 0]);
                central.extend(0u32.to_le_bytes());
                central.extend(0u32.to_le_bytes());
                central.extend(0u32.to_le_bytes());
                central.extend((name.len() as u16).to_le_bytes());
                central.extend([0u8; 8]);
                central.extend((0o100644u32 << 16).to_le_bytes());
                central.extend(off.to_le_bytes());
                central.extend(name.as_bytes());
            }
            let cd_off = out.len() as u64;
            let cd_size = central.len() as u64;
            out.extend(&central);
            drop(central);
            let rec = out.len() as u64;
            out.extend(0x0606_4b50u32.to_le_bytes());
            out.extend(44u64.to_le_bytes());
            out.extend([45, 0, 45, 0]);
            out.extend([0u8; 8]);
            out.extend(n.to_le_bytes());
            out.extend(n.to_le_bytes());
            out.extend(cd_size.to_le_bytes());
            out.extend(cd_off.to_le_bytes());
            out.extend(0x0706_4b50u32.to_le_bytes());
            out.extend(0u32.to_le_bytes());
            out.extend(
                (if std::env::var("STALE").is_ok() {
                    0
                } else {
                    rec
                })
                .to_le_bytes(),
            );
            out.extend(1u32.to_le_bytes());
            out.extend(0x0605_4b50u32.to_le_bytes());
            out.extend([0u8; 4]);
            out.extend(5u16.to_le_bytes());
            out.extend(5u16.to_le_bytes());
            out.extend(u32::MAX.to_le_bytes());
            out.extend((cd_off as u32).to_le_bytes());
            out.extend(0u16.to_le_bytes());
            eprintln!("archive {} bytes", out.len());
            let base = reset();
            let t = Instant::now();
            let r = Vfs::from_zip_bytes(out, ZipLimits::default());
            eprintln!(
                "RESULT {:?} in {:?}, extra peak {:.1} MiB",
                r.err(),
                t.elapsed(),
                extra(base)
            );
        }
        "nb" => {
            let mib = vec![0u8; 1 << 20];
            let names: Vec<String> = (0..500).map(|i| format!("z{i}")).collect();
            let files: Vec<(&str, &[u8])> =
                names.iter().map(|n| (n.as_str(), mib.as_slice())).collect();
            let zip = deflated_zip(&files);
            eprintln!("archive {} bytes", zip.len());
            let base = reset();
            let t = Instant::now();
            let r = Vfs::from_zip_bytes(zip, ZipLimits::default());
            eprintln!(
                "RESULT {:?} in {:?}, extra peak {:.1} MiB",
                r.err(),
                t.elapsed(),
                extra(base)
            );
        }
        "nc" => {
            let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
            let mut c = b"IHDR".to_vec();
            c.extend(8192u32.to_be_bytes());
            c.extend(8192u32.to_be_bytes());
            c.extend([16, 6, 0, 0, 0]);
            png.extend(13u32.to_be_bytes());
            png.extend(&c);
            png.extend(crc32(&c).to_be_bytes());
            for ty in [&b"IDAT"[..], b"IEND"] {
                png.extend(0u32.to_be_bytes());
                png.extend(ty);
                png.extend(crc32(ty).to_be_bytes());
            }
            let gltf = br#"{"asset":{"version":"2.0"},"images":[{"uri":"t.png"}],"textures":[{"source":0}],"materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}]}"#;
            let vfs = Vfs::from_zip_bytes(
                stored_zip(&[("t.gltf", gltf), ("t.png", &png)]),
                ZipLimits::default(),
            )
            .unwrap();
            let base = reset();
            let t = Instant::now();
            let r = load_gltf(&vfs, "t.gltf");
            eprintln!(
                "RESULT {:?} in {:?}, extra peak {:.1} MiB",
                r.err(),
                t.elapsed(),
                extra(base)
            );
        }
        "nd" => {
            let dir = temp_dir("probe_nd");
            std::fs::write(dir.join("big.bin"), vec![1u8; 200 << 20]).unwrap();
            std::fs::write(
                dir.join("t.gltf"),
                br#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":1,"uri":"big.bin"}]}"#,
            )
            .unwrap();
            let vfs = Vfs::open(&dir).unwrap();
            let base = reset();
            let t = Instant::now();
            let r = load_gltf(&vfs, "t.gltf");
            eprintln!(
                "RESULT {:?} in {:?}, extra peak {:.1} MiB",
                r.as_ref().err(),
                t.elapsed(),
                extra(base)
            );
            std::fs::remove_dir_all(&dir).unwrap();
        }
        _ => {}
    }
}
