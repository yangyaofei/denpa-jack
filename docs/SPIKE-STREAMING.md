# Spike: 流式 ASR 统一路线（2026-09-11）

## 事实（L4 官方文档，逐站核对）
1. **GLM-ASR 无真流式**: 唯一接口=POST 文件上传(multipart ≤25MB/30s)；`stream=true` 只是 SSE **响应分块**(transcript.text.delta/done)——录音边传边出的 partial 做不到
2. **GLM-Realtime 才是真流式 WSS**(wss://open.bigmodel.cn/api/paas/v4/realtime): 事件名与 OpenAI Realtime 高度一致(session.update/input_audio_buffer.append/conversation.item.input_audio_transcription.completed)——但它是**语音大模型产品**(glm-realtime 系列)非 ASR, 转写文档自述"可能为空/仅作参考"+按 Realtime 计价(价格较高)
3. 结论: **"智谱 ASR 走 OpenAI 兼容流式"不成立**(否决)。真流式只有: 豆包(已可用)/OpenAI Realtime(引擎已写好待 key)/本地自建

## 本地 Qwen3-ASR 路线(用户目标)——架构可行性
- 引擎端已就绪: Rust 引擎协议(start/feed/finish→Partial/Result/Error)与传输解耦; doubao/openai_realtime 两个流式实现可参照
- 本地方案=**本地 server 包装 Qwen3-ASR 为流式协议**(客户端分帧送入 PCM, server 分片推理回 partial)——与云引擎同构, Rust 端零架构改动
- 待验证(下一 spike): ①Qwen3-ASR 开源权重存在性+许可 ②M 系 Mac 推理 RTF(用户: 只要速度够快就淘汰线上) ③server 包装选型(sherpa-onnx? mlx? transformers?)

## Qwen3-ASR 本地路线查证（L4/L5）
- **已开源**(2026-01-29): 0.6B/1.7B, Apache-2.0, safetensors, HF/ModelScope 官方 QwenLM/Qwen3-ASR
- **Mac 原生路径**: moona3k/mlx-qwen3-asr —— M4 Pro 0.6B 4bit **RTF 0.018**(10s 音频 0.17s, 55x 实时); llama.cpp 官方 GGUF; sherpa-onnx(SenseVoice RTF≈0.11 作对照)
- **结论: 可行且速度远超"够快"门槛**——RTF 0.018 意味着 200ms 分片即时重推理全程余量充足
- **架构定案**: 本地 Python server(mlx-qwen3-asr)暴露流式协议(分片送入 PCM→窗口重推理→partial 修正→finish 终稿), Rust 引擎加 "local" provider(与 doubao 同构), 彻底移除线上依赖
- 下一步: spike P1=启动 mlx-qwen3-asr server+最小客户端实测 RTF; P2=Rust local provider

## 修正(用户指正, HF 原始文件核验): Qwen3-TTS 已开源
- Collection: huggingface.co/collections/Qwen/qwen3-tts —— 12Hz-0.6B/1.7B × Base/CustomVoice/VoiceDesign, Apache-2.0, 架构 Qwen3TTSForConditionalGeneration(多码本 LM+12Hz codec+speaker_encoder)
- 修正前述"TTS 闭源"结论; 计算量判据更新: 12Hz codec 把自回归步数压到 12 token/s(传统 25-86), 本地可行；单位时间计算量仍高于 ASR（多码本 + vocoder），但同一技术栈可实现
- 本地 TTS 选型更新: 首选 Qwen3-TTS-0.6B(与 Qwen3-ASR 同栈), Kokoro 降为备选
