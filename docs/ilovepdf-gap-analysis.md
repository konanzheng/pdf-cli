# iLovePDF 能力盘点 → pdf-cli 差距分析与选型建议

> 目标：找出 **适合 CLI 形态、适合 AI Agent 调用** 的新增功能。
> 结论优先：**先做 4 个"AI 原生"命令（search / pdf→markdown / diff / form），
> 而不是去补 iLovePDF 的 Office 转换全家桶。**

---

## 1. iLovePDF 的能力全貌（真实盘点）

站点首页 + API 文档（<https://www.iloveapi.com/docs/api-reference>）一共暴露 28 个 PDF 工具 + 9 个图像工具 + 一套电子签名。按语义归 6 类：

### 1.1 Organize（组织页面）
`merge` `split`（ranges / fixed_range / remove_pages / filesize 四种模式）`extract pages`
`organize`（拖拽排序、插页）`scan to pdf`（手机拍照→PDF，含去底色/边缘检测）

### 1.2 Optimize（优化）
`compress`（extreme / recommended / low 三档）`repair`（损坏恢复）`ocr`（**80+ 语言**，产可搜索 PDF）

### 1.3 Convert（转换）
**to PDF**：`jpg` `word` `powerpoint` `excel` `html`
**from PDF**：`pdf→jpg`（pages 渲染 / extract 抽图两模式）`pdf→word` `pdf→ppt` `pdf→excel` `pdf→pdfa` `pdf→markdown`

### 1.4 Edit（编辑）
`rotate` `page numbers`（facing_pages / first_cover / pages 子集 / starting_number）
`watermark`（文字+图片、9 宫格位置、透明度、铺满）`crop` `edit pdf`（加文字/图形/手绘批注）
`forms`（**检测已有表单域 / 新建可填表单 / 自主填写**）

### 1.5 Security（安全）
`unlock` `protect`（含权限位）`sign`（电子签名，含审计报告）`redact`（**真删内容**，非涂黑）`compare`（并排对比）

### 1.6 Intelligence（智能，新增，iLovePDF 的护城河）
`AI summarizer` `translate pdf`（保版式）`pdf→markdown`

### 1.7 平台型能力（非单个工具，但对 Agent 很关键）
- **Workflows**：把多个工具串成可复用流水线
- `/start → /upload → /process → /download` 任务式 API + `webhook` 异步回调
- `output_filename` / `packaged_filename` 模板（`{n}` `{date}` `{filename}` `{tool}`）
- `ignore_errors` / `try_pdf_repair` / `file_encryption_key`（客户端加密上传）
- 多区域（eu/us/fr/de/pl/in/sg）

---

## 2. 当前 pdf-cli 的覆盖情况

| 能力 | pdf-cli | 说明 |
|---|---|---|
| merge / split / delete-pages / rotate | ✅ | `split` 只支持 ranges + 逐页，缺 `--every N` / 奇偶 / 按大小 |
| pdf→image | ✅ | pdfium 渲染 |
| extract-images | ✅ | 原图直抽，比 iLovePDF 的 `extract` 模式更细（`--min-size`） |
| compress | ⚠️ | 只有 `prune + recompress`，**`--level` 三档参数当前是装饰性的**（实测 extreme 与 low 输出字节一致） |
| ocr | ⚠️ | 依赖外部 ocr-rust HTTP 服务，语言固定 |
| watermark add/remove | ✅ | 文本水印；另有 iLovePDF 完全没有的"烧录水印去除" |
| pagenum | ⚠️ | 只有 Helvetica + WinAnsi，**中文页码实测输出乱码**（`第 1 页` → `ç¬¬ 1 é¡µ`） |
| meta / info / doctor | ✅ | `doctor` 是差异化亮点 |
| encrypt / decrypt | ⚠️ | lopdf 只走 RC4(128)，AES-256(R6) 文件打不开 |
| convert（图/md/txt→pdf）| ✅ | 纯 Rust，无 Office |
| web-to-pdf | ✅ | headless Edge |
| **search** | ❌ | 无 |
| **pdf→markdown** | ❌ | 无 |
| **crop** | ❌ | 无 |
| **reorder / organize** | ❌ | 无 |
| **diff / compare** | ❌ | 无 |
| **redact** | ❌ | 无 |
| **form（检测/填写/展平）** | ❌ | ROADMAP 里明确"不做" |
| **repair** | ❌ | 无 |
| **pdf→pdfa** | ❌ | 无 |
| **pdf→word/ppt/excel** | ❌ | ROADMAP 明确"不做"（保真度）|
| **批量/递归/通配符** | ❌ | 所有命令单文件、`--input "a,b"` 逗号拼接 |
| **stdin/stdout 管道** | ❌ | 实测 `--input -` 直接报 FILE_NOT_FOUND |

---

## 3. 判断标准：什么叫"适合 CLI、适合 AI 用"

iLovePDF 是**人点鼠标**的产品，pdf-cli 是**Agent 调用**的工具。两者的最优功能集不一样。用 5 条标准筛：

| 标准 | 含义 | 反例 |
|---|---|---|
| **S1 可判定** | 输出能被程序校验（True/False、坐标、字节数、结构化列表） | "AI 摘要好不好" → 不可判定 |
| **S2 无需人眼** | 不依赖人类审美判断 | "edit pdf 拖放排版" |
| **S3 信息密度高** | 把非结构化 PDF 变结构化数据，省 Agent 的 token | `doctor` 就是正例 |
| **S4 纯本地** | 不上传云端 = 隐私 + 无额度 + 可离线 | 翻译/摘要必然要调模型，但它可以调**本地**模型 |
| **S5 组合性好** | 输出能直接喂给下一条命令 | `search --json` → `crop/redact --pages` |

按这 5 条打分（★ 越多越该做）：

| 功能 | S1 | S2 | S3 | S4 | S5 | 结论 |
|---|---|---|---|---|---|---|
| `search` 正则检索 | ★★★ | ★★★ | ★★★ | ★★★ | ★★★ | **P0** |
| `to-markdown` | ★★★ | ★★★ | ★★★★ | ★★★ | ★★★ | **P0** |
| `diff` | ★★★ | ★★★ | ★★★ | ★★★ | ★★ | **P0** |
| `form` 检测/填写/展平 | ★★★ | ★★★ | ★★★ | ★★★ | ★★ | **P1** |
| `crop` | ★★★ | ★★ | ★ | ★★★ | ★★★ | **P1** |
| `organize`(reorder/insert) | ★★★ | ★★★ | ★ | ★★★ | ★★★ | **P1** |
| `redact` | ★★ | ★★ | ★★ | ★★★ | ★★ | P2 |
| `repair` | ★★ | ★★★ | ★ | ★★★ | ★★ | P2 |
| `pdf→pdfa` | ★★ | ★★★ | ★ | ★★★ | ★ | P2 |
| OCR 增强（语言/可搜索PDF） | ★★★ | ★★★ | ★★★ | ★★ | ★★ | P1 |
| `batch`/`stdin` | ★★★ | ★★★ | — | ★★★ | ★★★ | **P0（地基）** |
| AI 摘要 / 翻译 | ★ | ★ | ★★ | ★ | ★ | 不做（交给上层 Agent） |
| pdf→word/ppt/excel | ★ | ★ | ★★ | ★★★ | ★ | 不做（保真度无解） |
| 电子签名 | ★★ | ★★ | — | ★★★ | ★ | 不做（证书链 + 审计体系，非核心） |

**关键洞察**：iLovePDF 的 6 类里，"Convert / Edit / Security" 大多已经被覆盖或不该追；
真正值得抄的是 **Intelligence 类的思路**——但不要抄它的"调大模型"，
而是把 `pdf→markdown` 做成**纯结构转换**（本地、确定性），摘要/翻译留给调用方。
**这才是给 AI 用的正确形态**：工具负责"把 PDF 变成模型能读的东西"，模型负责"想"。

---

## 4. 建议新增功能（按优先级）

### P0-1 `search` — 正则/字面量检索，返回坐标与上下文

最高性价比。`extract` 只给整篇文本，Agent 想知道"这句话在第几页、什么位置"必须靠猜。
iLovePDF 没有这个工具，但它是 Anthropic/OpenAI 文档工具链里使用频率最高的原语。

```
pdf-cli search --input doc.pdf --pattern "发票号码[:：]\s*(\d+)" --regex --json
```
```jsonc
{"success": true, "command": "search",
 "details": {
   "matches": [
     {"page": 3, "text": "发票号码：12345678", "groups": ["12345678"],
      "bbox": [72.0, 640.2, 210.5, 655.8], "context": "…购买方名称：XX公司…"}
   ],
   "total": 1, "pages_searched": 12 }}
```

- 实现：`lopdf` 解出页面文本（已有 `pdf-extract`），**坐标**由 pdfium 的
  `PdfPageText::search()` / `chars()` 提供（`pdfium-render` 已具备，仅需 pdfium.dll，与 `pdf-to-image` 同依赖）
- 无 pdfium 时降级：只给页码 + 上下文，`bbox: null` —— 保证命令永远可用
- 选项：`--regex` `--ignore-case` `--pages` `--max-matches` `--context N`
- 输出：`--count-only` 只回数量，`--json-lines` 流式（大文档不爆内存）

**为什么给 AI**：`grep` 的语义 Agent 天生会用；有了 bbox，`redact`/`crop`/`watermark` 就能精确落点。

### P0-2 `to-markdown` — PDF → 结构化 Markdown

iLovePDF 把它当付费 AI 卖点，但**90% 的工作是确定性排版还原**：按字号聚类成标题层级、
按 x 缩进判断列表、按 ruling line 拼表格。这正是 Agent 最缺的"喂给模型的干净输入"。

```
pdf-cli to-markdown --input doc.pdf --output doc.md
pdf-cli to-markdown --input doc.pdf --pages 1-10 --keep-images ./assets --json
```

- 标题：字号 + 加粗 + 独立行 → `#`~`######`
- 列表：行首符号/编号 + 左缩进对齐
- 表格：同一 y 带的文本段 × 列间距 → GFM 表格（pdfium `PdfPageTextObject::bounds`）
- 图片：复用现有 `extract-images`，落盘后写 `![](assets/p1_img1.png)`
- 页分隔：`---` + HTML 注释 `<!-- page 3 -->`（保页码可回溯）
- `--mode plain` 输出纯文本（给不做 Markdown 的模型）

**为什么给 AI**：这是 token 效率最高的 PDF 读入格式，且**完全离线、可复现**。
比 `extract` 强在"结构"，比"AI 转换"强在"免费且确定性"。

### P0-3 `diff` — 两版 PDF 结构化对比

ROADMAP 里已标 P1（"最高需求、工具稀缺"），这里给出**可判定**的输出契约：

```
pdf-cli diff --input old.pdf --input2 new.pdf --json
pdf-cli diff --input old.pdf --input2 new.pdf --mode render --output-dir ./diff
```
```jsonc
{"summary": {"pages_added": [12], "pages_removed": [], "pages_changed": [3, 7]},
 "text": [{"page": 3, "op": "replace", "old": "总计 100", "new": "总计 120"}],
 "render": [{"page": 3, "diff_ratio": 0.041, "mask": "diff_p003.png"}]}
```

- `--mode text`（默认）：按页做行级 LCS diff，纯 Rust，无 pdfium
- `--mode render`：pdfium 两侧渲染 → 像素异或 → 差异掩码 PNG，给"图变了但文字没变"的扫描件
- `--mode both`；`--threshold` 控制渲染差异灵敏度
- 文本层先归一化（去空白/页码漂移），避免改页码就全红

**为什么给 AI**：Agent 校验"我改的文件是否只改了该改的"，这是**自动化闭环的验收开关**。
没有 `diff`，AI 改 PDF 就是盲操作。

### P0-4 地基改造（不是新命令，但决定上限）

这三件事比多加三个命令更值钱：

1. **`--input` 支持目录/通配符 + `--output-dir` 映射**
   ```
   pdf-cli compress --input "docs/**/*.pdf" --output-dir out/ --recursive --jobs 4
   ```
   输出 `{"total": 42, "ok": 41, "failed": [{"file": "...", "code": "PARSE_FAILED"}]}`，
   单文件失败不中断整批（iLovePDF 的 `ignore_errors` 思路）。
2. **stdin/stdout 管道**
   `--input -` / `--output -`，让 `extract | grep`、`convert | merge` 成立；
   `--json-lines` 流式输出，避免大文档一次性进上下文。
3. **`--dry-run` 补齐**：`extract` / `info` / `doctor` 当前没有 dry_run 分支（只读命令可忽略），
   但 `merge`/`split` 的 dry-run 只报数量、不报**将写入的文件名**——Agent 需要预知落盘路径。

### P1-1 `form` — 表单域检测 / 填写 / 展平

ROADMAP 写"不做"的理由是"需 pdfium 表单 API"。但 `pdfium-render` **已经暴露了**：
`PdfFormField` + `as_text_field_mut().set_value()`、`as_checkbox_field_mut().set_checked()`、
`page.flatten()`、`PdfDocument::form_type()` / `field_values()`。

Agent 填表是高频刚需（合同、申请表），且**结果可判定**（字段值逐项比对）。

```
pdf-cli form --input blank.pdf --list --json
pdf-cli form --input blank.pdf --set "姓名=张三" --set "同意=true" --output filled.pdf
pdf-cli form --input filled.pdf --flatten --output flat.pdf
```

**建议推翻 ROADMAP 的"不做"结论**：成本低于 `pdf→word`，价值高于 `grayscale`。

### P1-2 `crop`

```
pdf-cli crop --input doc.pdf --box 40,40,-40,-40 --output cropped.pdf      # 四边内缩 pt
pdf-cli crop --input doc.pdf --box 0,0,595,400 --pages 1-3 --output c.pdf  # 绝对坐标
pdf-cli crop --input scan.pdf --auto --json                                # 自动去白边，报检测框
```
`--auto` 用 pdfium 渲染低分辨率页 → 找非白像素包围盒，**无 pdfium 时退化为 `--box` 必填**。
扫描件去白边是 OCR 前的标准动作，Agent 流水线会用。

### P1-3 `organize` — 页面重排/复制/插入

```
pdf-cli organize --input doc.pdf --order 3,1,2,1-5,3 --output out.pdf
pdf-cli organize --input doc.pdf --insert 2:cover.pdf --output out.pdf
```
`--order` 复用现有页码语法并扩展（允许重复 → 实现"复制页"），
这比 `reorder` + `insert` 两个命令更省心，也贴合 iLovePDF 的 Organize 心智模型。

### P1-4 `split` 补齐三模式 + `merge` 保书签

- `--every N`（每 N 页一份）、`--odd-even`（双面扫描拆分）、`--max-size`（按体积切，iLovePDF 的 `filesize` 模式）
- `merge` 迁移大纲：`lopdf` 有 `outlines.rs` / `toc.rs`，可读取每个源 PDF 的 `Outlines` 并按页偏移重写到合并文档
- 这两条 ROADMAP 已列，只是没排期；`--max-size` 是新增想法

### P1-5 OCR 增强

- `--lang chi_sim+eng` 透传给 ocr-rust（服务端支持则生效，不支持则明确报错而非静默）
- `--output-pdf`：把 OCR 文本层**写回** PDF（用 `lopdf` 加不可见文本层 `Tr 3`），
  产出"可搜索 PDF"——这正是 iLovePDF `ocr` 的核心交付物，当前 pdf-cli 只输出 txt
- 与 `to-markdown` 组合：扫描件 → OCR → Markdown

### P2（可选，成本/收益比一般）

| 功能 | 说明 | 注意 |
|---|---|---|
| `redact` | 按 `search` 的 bbox 涂黑并**真删**内容流对象 | 必须同时删文本，否则等于没删；用 `--verify` 复检 |
| `repair` | `qpdf`/`mutool` 无纯 Rust 等价物；先做"宽松解析 + 忽略坏对象重建 xref" | 成功率有限，务必 `--report` 说明修了什么 |
| `pdf→pdfa` | 需嵌入 ICC + 字体嵌入 + 禁透明度 | 工作量大，先做 `--font` 字体嵌入更划算 |
| `encrypt` AES-256 | lopdf 的 V5/R6 已实现，但当前命令可能只走 RC4 | 先查清 `security.rs` 用了哪档，补 `--aes256` |
| `grayscale` | 图像转灰度后回写 | 与 `compress` 合并成 `--grayscale` 选项更好 |
| `pdf→html` | 保真度有限 | 收益低于 `to-markdown` |

### 明确不建议做（保持现有判断）

- **AI 摘要 / 翻译**：不可判定、需模型、Agent 上层自己就能做。工具应提供"干净的输入"（`to-markdown`）而非"结论"。
- **pdf→word/ppt/excel**：保真度是行业难题，iLovePDF 靠长期投入；ROADMAP 的判断正确。
- **电子签名**：证书链 + 长时验证 + 审计报告是一整套体系，非单人项目合理范围。
- **PDF 编辑器（拖放文字/图形）**：交互式，违背非交互契约。

---

## 5. 顺带发现的问题（修复成本低，影响大）

按实测结果，这些是**现有命令的坑**，建议在加新功能前先修：

| # | 问题 | 实测证据 | 修法 |
|---|---|---|---|
| 1 | `pagenum` 中文乱码 | `第 1 页` → `第 1 页` 输出为 `ç¬¬ 1 é¡µ` | Helvetica+WinAnsi 不支持 CJK；需嵌入 TrueType 子集（`ab_glyph` 已在本地缓存 + `printpdf`/手工 CIDFont）。**这是本土化核心卖点**，ROADMAP 已标 P2，建议提到 P1 |
| 2 | `compress --level` 无效 | `extreme` 与 `low` 输出均为 31300 字节 | 参数未接线。真正的档位应控制：图像重采样/JPEG 质量、`--level extreme` 走"渲染降分辨率"，`low` 只做对象清理 |
| 3 | `split` 输出名固定 `stem_1.pdf` | 逐页拆分不体现页码，批量时易错配 | 改为 `stem_p001.pdf`，或 `--name-template "{stem}_{n:03}"` |
| 4 | 加密 PDF 错误码不准 | 无密码调 `extract` → `PARSE_FAILED`，而 `PASSWORD_REQUIRED` 从未被抛出 | lopdf 的 decryption error 应映射为 `PASSWORD_REQUIRED` |
| 5 | `extract --pages` 静默错误 | 页范围越界/非法时返回空文本、退出码 0 | 越界应报 `INVALID_PAGE_RANGE`；解析失败不应静默跳过 |
| 6 | 无 `--password` 全局参数 | 任何命令遇到加密 PDF 只能失败 | 加全局 `--password`，入口统一 `authenticate_password` |
| 7 | 错误信息缺结构化 hints | `\n  → ...` 混在 message 里，Agent 要二次解析 | `details` 增加 `hint` / `suggestion` / `retry_with` 字段 |

---

## 6. 建议的落地顺序

```
第 1 批（地基，1 个迭代）
  --input 目录/通配符 + --output-dir + 批量报告    ← 所有后续命令直接受益
  stdin/stdout + --json-lines
  全局 --password + 错误码修正 + hint 字段
  修 pagenum 中文字体（嵌入子集）

第 2 批（AI 原生三件套）
  search --regex --json（bbox 可选）
  to-markdown
  diff --mode text|render

第 3 批（页面手术）
  crop --box|--auto
  organize --order|--insert
  split --every|--odd-even|--max-size
  merge 保书签

第 4 批（进阶）
  form --list|--set|--flatten
  ocr --output-pdf（可搜索 PDF）
  redact --verify
  compress 真实档位 + --grayscale
```

**一句话选型**：iLovePDF 卖的是"人能点的按钮"，pdf-cli 该卖的是
**"Agent 能判断、能组合、能闭环"的原语** —— `search` 给它眼睛，`to-markdown` 给它干净的输入，
`diff` 给它验收开关，`form` 给它可执行的落点。这 4 个的边际价值远高于再补 3 个格式转换器。
