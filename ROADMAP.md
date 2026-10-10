# pdf-cli Roadmap

基于用户需求调研（StackExchange 260 条问答 + CSDN + GitHub Issues）规划。

## 已完成（v0.1.0）

转换 / 页面 / 编辑 / 安全 / 诊断 / 办公 共 15 个命令，详见 README。

## v0.2.0 — 本批

| 命令 | 说明 |
|---|---|
| `pagenum` | 加页码（位置/格式/起始页可调） |
| `meta` | 编辑元数据（标题/作者/主题/关键词/创建者/生产者） |
| `extract-images` | 提取内嵌原图（保留原始格式，不重渲染） |
| `ocr` | 扫描件 OCR（调用 ocr-rust HTTP 服务，pdfium 渲染页面） |
| `watermark-remove` | 去除**烧录进图像**的水印（扫描/栅格页，尽力版，见下） |

### 关于 `watermark-remove`（重要：范围与边界）

原始调研把「去水印」列为不做，理由是**矢量/文本层水印**（作为独立 PDF 对象存在，
如 `watermark-add` 生成的那种）需解析内容流去识别水印对象，**基本不可靠**。

后续实践发现另一类高频场景：**扫描件/栅格页**——页面本身就是一张整页图
（Image XObject，`DCTDecode` JPEG），水印是渲染进像素里的浅色文字（如"仅限XX项目用"
平铺水印）。这类水印与正文存在**亮度/饱和度差异**，可以用图像阈值干净分离，
因此作为**独立的尽力版命令**补充实现：

- 遍历页面 Image XObject，把「亮度 ≥ `--threshold` 且 饱和度 ≤ `--max-sat`」的像素涂白
- 默认 `--threshold 212 --max-sat 12`，保留深色正文与彩色印章
- 仅处理 `DCTDecode` 的 RGB/Gray 图像；找不到可处理图像时**报错而非原样输出**

**仍然不做**：矢量 / 文本层水印的去除（无可靠通用方案）。

## v0.3.0 — 计划中

| 命令 / 功能 | 说明 | 优先级 |
|---|---|---|
| `diff` | PDF 对比（文本层 + 渲染层）— 报告最高需求，工具稀缺 | P1 |
| `search` | 正则搜索 + 命中页码 | P1 |
| `reorder` | 页面重排 `--order 3,1,2` | P1 |
| `crop` | 裁剪页边距 / 自定义 CropBox | P1 |
| `split --every N` | 每 N 页一份 | P1 |
| `split --odd-even` | 双面扫描拆分（奇偶页分开） | P1 |
| 批量/递归 | `--input` 支持目录 / 通配符，全局 `--recursive` | P1 |
| `repair` | 尝试修复损坏 PDF（成功率有限） | P2 |
| `grayscale` | 灰度化 | P2 |
| `encrypt --permissions` | 精细权限（禁复制/禁打印） | P2 |
| `convert --font` | 嵌入中文字体（解决中文乱码） | P2 |
| `merge` 保书签 | 合并时迁移大纲/书签 | P2 |
| `pdf-to-html` | PDF → HTML（保真度有限） | P3 |

## 已明确不做

| 功能 | 原因 |
|---|---|
| 矢量/文本层水印去除 | 需识别内容流中的水印对象，**基本不可靠**；用户反馈 pdftk 也去不干净。**注**：烧录进图像的扫描件水印已由 `watermark-remove` 覆盖（尽力版） |
| `sign`（数字签名） | 需证书链 + PDF 签名规范，工作量大，非核心场景 |
| `form`（表单填充/展平） | 需 pdfium 表单 API，场景较窄 |
| `pdf-to-word` | 保真度是行业难题（报告明确列为最大技术难点），投入产出比低 |

## 集成说明

### OCR

`ocr` 命令调用同机的 **ocr-rust HTTP 服务**（独立项目，PP-OCRv6 ONNX 模型）：

    # 先启动 OCR 服务
    cd F:\rust\ocr-rust
    .\target\release\ocr-rust.exe     # 默认端口 18004

    # 再用 pdf-cli 调用
    pdf-cli ocr --input scan.pdf --service http://localhost:18004 --output text.txt

pdf-cli 负责：pdfium 渲染页面 → PNG → POST 到 `/ocr/raw` → 汇总文本。
不做模型内置，避免二进制膨胀（模型 ~10MB+）与依赖冲突。

### 运行时依赖汇总

| 命令 | 依赖 |
|---|---|
| `pdf-to-image`, `ocr` | `pdfium.dll` |
| `web-to-pdf` | Microsoft Edge |
| `ocr`（额外） | ocr-rust 服务运行中 |

## 战略方向（来自调研）

1. **批处理 / 命令行优先** — 用户反复在找 CLI 工具；后续重点是目录递归与通配符
2. **中文零乱码** — 字体嵌入是本土化核心卖点（`convert --font`）
3. **保留结构** — 书签、目录、元数据不丢（`merge` 增强）
