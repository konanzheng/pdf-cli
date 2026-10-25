# pdf-cli 实现契约（多 agent 并行开发共享规范）

本文件是所有并行开发 agent 的**唯一约定来源**。实现任何新命令前先读本文件。

## 1. 文件所有权（硬性规则，违反会导致合并冲突）

主 agent 独占，**任何人不得修改**：

| 文件 | 原因 |
|---|---|
| `src/main.rs` | CLI 定义与 dispatch，已全部预先写好 |
| `src/commands/mod.rs` | 模块声明，已全部预置 |
| `src/output.rs` | `CommandResult` 契约 |
| `src/error.rs` | `PdfError` 契约 |
| `src/io.rs` | 共享 I/O 工具，只读调用 |
| `Cargo.toml` | 依赖已预置齐全 |

**你只允许写你自己的那一个文件。** 如果你需要 `io.rs` / `error.rs` 里没有的能力，不要改它们，改为在你自己的文件内用私有辅助函数实现，并在最终报告里列出"建议主 agent 后续合并到 io.rs 的部分"。

## 2. 已可用的共享能力（`src/io.rs`，直接调用，不要复制）

```rust
use crate::io;

io::is_stdio(path: &str) -> bool              // path == "-"
io::read_bytes(input: &str) -> Result<Vec<u8>, PdfError>     // 支持 stdin "-"
io::read_text(input: &str) -> Result<String, PdfError>
io::write_bytes(output: &str, data: &[u8]) -> Result<(), PdfError>   // 支持 stdout "-"
io::write_text(output: &str, text: &str) -> Result<(), PdfError>     // 写文件时自动补换行
io::load_doc(input: &str, password: Option<&str>) -> Result<lopdf::Document, PdfError>
io::load_doc_bytes(bytes: &[u8], password: Option<&str>) -> Result<lopdf::Document, PdfError>
io::parse_page_range(spec: Option<&str>, total: u32) -> Result<Vec<u32>, PdfError>  // 严格解析
io::expand_inputs(input: &str) -> Result<Vec<PathBuf>, PdfError>     // 单文件/目录/glob
io::bind_pdfium(pdfium_path: Option<&str>) -> Result<Pdfium, PdfError>
io::with_extension(path: &Path, ext: &str) -> PathBuf
io::stem_of(path: &Path) -> String
// 批量输入（逗号列表 / 目录 / glob，任一 token 展开失败即报错，不静默跳过）
io::expand_batch(input: &str) -> Result<Vec<PathBuf>, PdfError>
// 单文档输入：只接受恰好命中一个文件，命中 0 个或多个都报 OPERATION_FAILED
io::resolve_input_once(input: &str) -> Result<PathBuf, PdfError>
// 把 PathBuf 转回可回显、可再次传给 --input 的字符串
io::path_to_arg(path: &Path) -> String
```

**单文档 vs 批量**——这是新增命令时必须先定的一件事：

| 命令族 | 输入语义 | 用哪个 helper |
| --- | --- | --- |
| 单文档（`info`/`crop`/`form`/`pagenum`/`diff`/`organize`） | 必须恰好一个文件；目录或多种 glob 命中 >1 个时报错 | `resolve_input_once` |
| 批量（`extract`/`convert`/`merge`） | 逗号列表、目录、glob 都展开成多份逐个处理 | `expand_batch` |

**不要再自己写 `Path::exists()` 预检**：它会在输入展开之前就否掉目录和 glob。
让 `expand_inputs` / `resolve_input_once` 去判断，错误信息才带得上「命中了几个文件」。

`io::parse_page_range` 语义：`None` / 空串 = 全部页；支持 `1-3,5,7-9`、`3-`（到末页）、`-5`（从首页）；
越界、倒序、杂散逗号、非数字**一律返回错误**（`INVALID_PAGE_RANGE`），绝不静默丢弃。

从 `src/commands/pages.rs` 可复用（已 `pub`）：

```rust
crate::commands::pages::copy_objects(dst: &mut Document, src: &Document) -> BTreeMap<ObjectId, ObjectId>
crate::commands::pages::remap_refs(obj: &mut Object, map: &BTreeMap<ObjectId, ObjectId>)
```

`copy_objects` 会把 `src` 的全部对象深拷贝进 `dst` 并返回旧 id → 新 id 的映射，是"从别的文档搬页"的标准做法。

## 3. 错误契约（`src/error.rs`）

已有变体，**只使用已有的**，不要新增：

```rust
PdfError::FileNotFound { path }
PdfError::UnsupportedFormat { format }
PdfError::ParseFailed { reason }
PdfError::PasswordRequired
PdfError::WrongPassword { path }
PdfError::Operation { reason }              // 兜底，尽量少用
PdfError::InvalidPageRange { range }
PdfError::InvalidPageRangeReason { spec, reason }   // 优先用这个，带具体原因
PdfError::MissingArg { arg, example }
PdfError::BrowserNotFound { reason }
PdfError::PdfiumNotFound { reason }
```

`Operation { reason }` 的 reason 文案要**可执行**：写清"哪里失败、期望什么、下一步怎么办"，
例如 `"No text layer found; the PDF is scanned. Render pages with pdf-to-image and run a
separate OCR tool, then feed the text back"`，而不是 `"failed"`。

**不要**在错误提示里叫用户去跑一个不存在的命令。pdf-cli 不做 OCR，所以提示必须指向
外部工具或本仓库真实存在的命令（例如 `pdf-to-image`、`convert`）。

## 4. 输出契约（`src/output.rs`）

```rust
CommandResult::ok(command: &str, output: Option<String>, details: serde_json::Value)
```

- `command`：命令的 kebab-case 名，例如 `"search"`、`"to-markdown"`、`"diff"`、`"form"`、`"crop"`、`"organize"`。
- `output`：人类可读的一行摘要（非 `--json` 时打印它）。**不要**把整个结果塞这里。
  文件输出类命令（crop/organize/form set）放输出路径；纯分析类命令（search/diff）放一句摘要。
- `details`：**全部机器可读数据放这里**，用 `serde_json::json!` 构造。
  字段名用小写 snake_case；所有集合用数组；布尔用 `true/false`，不要字符串。

### 已确立的 `details` 字段（新增命令请沿用这些名字，不要另造同义词）

| 字段 | 出现于 | 含义 |
| --- | --- | --- |
| `resolved` | `info` | 输入是 glob/目录时，实际解析到的那个具体文件；单文件输入时等于 `input` |
| `strategy` | `split` | `"ranges"` / `"every"` / `"odd-even"` / `"per-page"`，说明按哪种规则切的 |
| `pages_per_file` | `split` | 每个输出文件包含的页数数组，与 `files` 等长 |
| `outline_merged` | `merge` | 是否写入了 `/Outlines` |
| `outline_items` | `merge` | 合并后书签条目总数（含嵌套） |
| `font_embedded` / `glyphs_subset` | `convert` | 是否内嵌字体、子集化的字形数 |
| `bom_stripped` | `convert` | 是否剥掉了输入开头的一个 U+FEFF |

**布尔字段的稳定性**：可选能力对应的键要**始终存在**（不支持时给 `null`/`false`），
不要「有这个功能才多一个键」——否则调用方无法区分「没这个字段」和「值为假」。

**已删除的字段**：v0.3.1 曾有 `ocr` 命令及其 `lang` / `output_pdf` / `searchable` /
`pages_with_text` 字段，该命令已整体移除。不要因为旧文档提到就把它加回来——OCR 属于
独立的 OCR 工具，见 ROADMAP.md「已移除：`ocr`」。

### `--dry-run` 约定

`dry_run == true` 时**不得写任何文件**，但要完整算出并返回将要执行的动作：

```json
{ "dry_run": true, ...其余字段照常填, 例如 "output": "out.pdf", "pages_affected": 3 }
```

## 5. 必须遵守的通用行为

1. **不交互**。绝不读终端输入、绝不等待确认。所有参数来自 CLI flag。
2. **确定性**。相同输入必须产出相同输出（文件名、顺序、JSON key 顺序）。集合先排序再输出。
3. **不吞错**。任何失败路径返回 `Err`，不 `return Ok` 假装成功。
4. **页码 1-based**，输出给用户看的页号也是 1-based。
5. **`--pages` 一律走 `io::parse_page_range`**，不要自己再写一遍解析。
6. **加密输入**：所有需要读 PDF 的地方用 `io::load_doc(input, password)`，不要直接
   `Document::load()`，否则加密文件会报成 `PARSE_FAILED` 而不是 `PASSWORD_REQUIRED`。
7. **路径**：输出目录不存在时 `std::fs::create_dir_all` 自动创建。
8. **日志只走 stderr**（`eprintln!`），且仅在被调到的时候；stdout 保留给结果。

## 6. 提交前自检

```powershell
cd F:\rust\cnb\pdf-cli
cargo check 2>&1 | Select-String "^(error|warning: unused)"
```

**注意**：多个 agent 共享同一个 target 目录，`cargo check` 可能输出 `Blocking waiting for file lock`
（正常，等即可），也可能报出**其他 agent 文件**里的错误——那是别人的问题，你只需要让自己的文件零错误零警告。
如果你的文件出现 `unused` 警告，要么用起来，要么删掉。

同时确认 CJK 内容没有乱码（写文件时务必 UTF-8 无 BOM）：

```powershell
(Get-Content <你的文件> -Raw -Encoding UTF8).ToCharArray() | Where-Object { $_ -eq [char]0xFFFD } | Measure-Object
# Count 必须为 0
```

## 7. 测试素材

`testdata\` 下已有：`txt.pdf`（纯文本小文件）、`numbered.pdf`（多页）、`web.pdf`（32 KB 内容较多）、
`inv2.pdf`/`inv3.pdf`（发票）、`enc.pdf`（加密）、`meta.pdf`、`rotated.pdf`、
`merged.pdf`、`img.pdf`、`svg.pdf`、`html.pdf`、`ocr_sample.pdf`（扫描件，无文本层）、
`small.pdf`、`wm.pdf`。

pdfium.dll 位于 `testdata\pdfium\bin\pdfium.dll`，需要时用
`--pdfium-path testdata\pdfium\bin\pdfium.dll` 或代码里 `io::bind_pdfium(Some(...))`。
