//! macOS の音の出力の遅れ（OS に聞く。OS の関数を呼ぶ unsafe はこの置き場所だけ：`audio_out` が使う）

/// 既定の出力の装置が受け取った音が耳に届くまでのサンプルの数（装置の速さで。macOS が知らせる、装置の遅れ・安全分・
/// 最初の出力の流れの遅れの和。Bluetooth の装置では大きい。cpal が開く既定の出力の装置と同じものを聞く）。聞けなければ 0
pub(crate) fn output_latency() -> u64 {
    use coreaudio_sys::{
        kAudioDevicePropertyLatency, kAudioDevicePropertySafetyOffset, kAudioDevicePropertyStreams,
        kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
        kAudioStreamPropertyLatency, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize,
        AudioObjectID, AudioObjectPropertyAddress,
    };
    let addr = |selector: u32, scope: u32| AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    };
    // 物 `obj` の性質 `a` の、32 ビットの値の並び
    let get = |obj: AudioObjectID, a: AudioObjectPropertyAddress| -> Vec<u32> {
        let mut size = 0u32;
        // SAFETY: 性質の大きさを聞くだけ（書く先は `size`）
        if unsafe { AudioObjectGetPropertyDataSize(obj, &a, 0, std::ptr::null(), &mut size) } != 0 {
            return Vec::new();
        }
        let mut v = vec![0u32; size as usize / std::mem::size_of::<u32>()];
        let mut size = (v.len() * std::mem::size_of::<u32>()) as u32;
        // SAFETY: 書く先は `v`（`size` バイト）
        let st = unsafe {
            AudioObjectGetPropertyData(obj, &a, 0, std::ptr::null(), &mut size, v.as_mut_ptr().cast())
        };
        if st != 0 {
            return Vec::new();
        }
        v.truncate(size as usize / std::mem::size_of::<u32>());
        v
    };
    let (global, out) = (kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput);
    let Some(&device) = get(kAudioObjectSystemObject, addr(kAudioHardwarePropertyDefaultOutputDevice, global)).first()
    else {
        return 0;
    };
    let first = |v: Vec<u32>| u64::from(v.first().copied().unwrap_or(0));
    let stream = get(device, addr(kAudioDevicePropertyStreams, out))
        .first()
        .map_or(0, |&s| first(get(s, addr(kAudioStreamPropertyLatency, global))));
    first(get(device, addr(kAudioDevicePropertyLatency, out)))
        + first(get(device, addr(kAudioDevicePropertySafetyOffset, out)))
        + stream
}
