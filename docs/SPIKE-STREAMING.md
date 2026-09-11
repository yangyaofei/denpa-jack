# Spike: 流式 ASR 统一路线（2026-09-11）

## 事实（L4 官方文档, subagent 全站核对, 详见 tmp/glm-asr-spike-notes.md）
1. **GLM-ASR 无真流式**: 唯一接口=POST 文件上传(multipart ≤25MB/30s)；`stream=true` 只是 SSE **响应分块**(transcript.text.delta/done)——录音边传边出的 partial 做不到
2. **GLM-Realtime 才是真流式 WSS**(wss://open.bigmodel.cn/api/paas/v4/realtime): 事件名与 OpenAI Realtime 高度同款(session.update/input_audio_buffer.append/conversation.item.input_audio_transcription.completed)——但它是**语音大模型产品**(glm-realtime 系列)非 ASR, 转写文档自述"可能为空/仅作参考"+按 Realtime 计价(贵)
3. 结论: **"智谱 ASR 走 OpenAI 兼容流式"不成立**(否决)。真流式只有: 豆包(已通)/OpenAI Realtime(引擎已写好待 key)/本地自建

## 本地 Qwen3-ASR 路线(用户目标)——架构可行性
- 引擎端已就绪: Rust 引擎协议(start/feed/finish→Partial/Result/Error)与传输解耦; doubao/openai_realtime 两个流式实现可参照
- 本地方案=**本地 server 包装 Qwen3-ASR 为流式协议**(客户端分帧喂 PCM, server 分片推理回 partial)——与云引擎同构, Rust 端零架构改动
- 待验证(下一 spike): ①Qwen3-ASR 开源权重存在性+许可 ②M 系 Mac 推理 RTF(用户: 只要速度够快就淘汰线上) ③server 包装选型(sherpa-onnx? mlx? transformers?)
