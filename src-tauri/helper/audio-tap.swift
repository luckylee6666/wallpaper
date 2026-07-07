import Foundation
import AVFoundation
import CoreMedia
import ScreenCaptureKit

// 用 ScreenCaptureKit 采集系统音频，向 stdout 写 48kHz 单声道 f32le PCM。
// 依赖"屏幕录制"权限（TCC 归属于父 App）。父进程关闭 stdin 即退出。
// 所有对 AudioBufferList 的读取都在 withAudioBufferList 闭包内完成（生命周期契约）。

final class AudioTap: NSObject, SCStreamOutput, SCStreamDelegate {
    let out = FileHandle.standardOutput
    var badStreak = 0

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer,
                of type: SCStreamOutputType) {
        guard type == .audio else { return }
        let ok: Bool = (try? sampleBuffer.withAudioBufferList { audioBufferList, _ -> Bool in
            guard let absd = sampleBuffer.formatDescription?.audioStreamBasicDescription,
                  let format = AVAudioFormat(standardFormatWithSampleRate: absd.mSampleRate,
                                             channels: absd.mChannelsPerFrame),
                  let pcm = AVAudioPCMBuffer(pcmFormat: format,
                                             bufferListNoCopy: audioBufferList.unsafePointer),
                  let ch = pcm.floatChannelData else { return false }
            let frames = Int(pcm.frameLength)
            let channels = Int(pcm.format.channelCount)
            guard frames > 0, channels > 0 else { return false }
            if channels == 1 {
                out.write(Data(bytes: ch[0], count: frames * 4))
            } else {
                var mono = [Float32](repeating: 0, count: frames)
                for c in 0..<channels {
                    let data = ch[c]
                    for i in 0..<frames { mono[i] += data[i] }
                }
                let inv = 1.0 / Float32(channels)
                for i in 0..<frames { mono[i] *= inv }
                mono.withUnsafeBufferPointer { p in
                    out.write(Data(buffer: p))
                }
            }
            return true
        }) ?? false

        if ok {
            badStreak = 0
        } else {
            // 格式不符导致的持续静默丢帧：明确退出，让 Rust 侧上报而不是无限挂起
            badStreak += 1
            if badStreak > 100 {
                FileHandle.standardError.write("unsupported audio format; exiting\n".data(using: .utf8)!)
                exit(4)
            }
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        // 睡眠唤醒/锁屏/显示器重配都会走到这里；Rust 侧负责自动重启
        FileHandle.standardError.write("stream stopped: \(error)\n".data(using: .utf8)!)
        exit(1)
    }
}

let tap = AudioTap()
// 唯一强引用，保证 SCStream 在 Task 闭包结束后不被释放——删掉它采集会静默停止
var streamRef: SCStream?

Task {
    do {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        guard let display = content.displays.first else {
            FileHandle.standardError.write("no display\n".data(using: .utf8)!)
            exit(2)
        }
        let filter = SCContentFilter(display: display, excludingWindows: [])
        let config = SCStreamConfiguration()
        config.capturesAudio = true
        config.excludesCurrentProcessAudio = true
        config.sampleRate = 48000
        config.channelCount = 1  // 直接要单声道，省去手动下混
        // SCK 必须带视频通道，降到最低成本后直接丢弃
        config.width = 2
        config.height = 2
        config.minimumFrameInterval = CMTime(value: 1, timescale: 1)
        let stream = SCStream(filter: filter, configuration: config, delegate: tap)
        try stream.addStreamOutput(tap, type: .audio, sampleHandlerQueue: DispatchQueue(label: "audio-tap"))
        try await stream.startCapture()
        streamRef = stream
    } catch {
        FileHandle.standardError.write("capture error: \(error)\n".data(using: .utf8)!)
        exit(3)
    }
}

// stdin EOF → 父进程退出或主动停止 → 结束自身
DispatchQueue.global().async {
    _ = try? FileHandle.standardInput.readToEnd()
    exit(0)
}

dispatchMain()
