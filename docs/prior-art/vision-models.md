# Vision models for scans and photos

Machine-learning models that could improve or extend what sheaf does with
scanned and photographed pages, beyond the OCR engines already in
`sheaf-ocr`. Researched 2026-10-14. Claims we could not confirm are marked
"(unverified)".

What sheaf already runs (see [`todos/cleanup-cli.md`](../../todos/cleanup-cli.md)):

- PaddleOCR PP-OCRv5 text detection and recognition, through `oar-ocr` on
  ONNX Runtime;
- PaddleOCR's document-orientation model, as one vote in auto-rotate;
- UVDoc for flattening curled pages: opt-in, about 31 MB, about 0.9 s per
  page on CPU;
- classical cleanup in `sheaf_image::clean`: deskew, despeckle, Sauvola
  black-and-white, and illumination correction that divides the page by its
  estimated background.

## 1. DocRes

**"DocRes: A Generalist Model Toward Unifying Document Image Restoration
Tasks"**, Jiaxin Zhang, Dezhi Peng, Chongyu Liu, Peirong Zhang, Lianwen Jin.
CVPR 2024, pp. 15654–15664.
[Paper](https://arxiv.org/abs/2405.04408) ·
[full text](https://arxiv.org/html/2405.04408v1) ·
[code](https://github.com/ZZZHANG-jx/DocRes) ·
[CVF page](https://openaccess.thecvf.com/content/CVPR2024/html/Zhang_DocRes_A_Generalist_Model_Toward_Unifying_Document_Image_Restoration_Tasks_CVPR_2024_paper.html)

### 1.1 What it is

One network that does five document restoration tasks. You choose the task
at inference time:

| Task | What it fixes | sheaf's current answer |
|---|---|---|
| Dewarping | curled or folded pages | UVDoc (opt-in) |
| Deshadowing | shadows cast on the page | background division in `clean` |
| Appearance enhancement | uneven lighting, color cast, paper tone | background division plus white balance |
| Deblurring | motion or focus blur on text | none |
| Binarization | degraded or low-contrast black-and-white | Sauvola |

The repo also has an `end2end` mode for camera captures. It runs the tasks
one after another, each pass taking the previous pass's output.

### 1.2 How it works

- **Backbone:** an unmodified [Restormer](https://arxiv.org/abs/2111.09881),
  with 15.2 M parameters and 183.0 GFLOPs (paper, supplementary Table 1).
  15.2 M fp32 parameters is about 61 MB of weights. The paper doesn't say
  what input size the GFLOPs figure assumes.
- **DTSPrompt** ("dynamic task-specific prompt"): for each task, a few prior
  channels are computed from the input with ordinary image processing and
  concatenated to the RGB image, giving 6 input channels. The prompt tells
  the network which task to do and adds information it can use:

  | Task | Prompt channels |
  |---|---|
  | Dewarping | document mask from a separate segmentation model (MARIOR's; the repo's `mbd.pkl`), plus x and y coordinate maps |
  | Deshadowing | the shadowed background: dilate to remove the text, then median filter |
  | Appearance | 255 − \|input − background\| |
  | Deblurring | gradient map, repeated to 3 channels |
  | Binarization | Sauvola output, Sauvola threshold map, gradient map |

  The deshadowing and binarization priors are things sheaf already
  computes: the background estimate in `clean`'s illumination step, and
  Sauvola. So the preprocessing for DocRes would mostly reuse our own code.
- **Resolution:** trained on 256×256 patches (dewarping on images resized to
  256×256). The paper says inference works at variable resolutions up to
  1600×1600, because the prompt isn't tied to ViT patching. Restormer's
  attention runs across channels rather than pixels, so its memory grows
  linearly with image size. A 300 dpi letter page (2550×3300) is still
  above 1600×1600 and would need tiling or downscaling (unverified how
  well tiles stitch).

### 1.3 Results

DocRes is close to the best task-specific models on each task, but not
ahead on all of them (paper, Table 1):

| Task (benchmark) | DocRes | Best task-specific model |
|---|---|---|
| Dewarping (DIR300) MS-SSIM / LD | 0.6264 / 6.83 | DocGeo 0.6380 / 6.40 |
| Deshadowing PSNR: Jung / Kligler / OSR | 23.02 / 27.14 / 21.64 | DocShadow 21.05 (Jung); BGSNet 29.17 (Kligler), 22.64 (OSR) |
| Appearance PSNR: RealDAE / DocUNet* | 24.65 / 17.60 | GCDRNet 24.42 / 17.09 (better SSIM on both) |
| Deblurring (TDD) PSNR / SSIM | **27.35 / 0.9723** | DocDiff 24.00 / 0.9559 |
| Binarization (DIBCO'18) FM | 89.82 | GDB 91.09 |

The authors list the limitations themselves:
- In end-to-end mode, errors build up from one pass to the next
  (supplementary Fig. 2).
- For deblurring and shadow removal, the prompt adds little over a fixed
  prompt.

There are no wall-clock timings.

### 1.4 Licensing and packaging

- **Code:** MIT, "Copyright (c) 2024 Jiaxin Zhang"
  ([LICENSE](https://github.com/ZZZHANG-jx/DocRes/blob/master/LICENSE)).
- **Weights:** `docres.pkl` and `mbd.pkl` are PyTorch pickles on OneDrive.
  The repo doesn't give a separate license for the weights (unverified
  whether the MIT license covers them). `mbd.pkl` comes from another
  project (MARIOR), so its license needs checking separately.
- **ONNX:** we found no published DocRes export. Restormer itself exports
  cleanly; the third-party
  [efwfe/restomer](https://huggingface.co/efwfe/restomer) repo has FP32 and
  FP16 ONNX exports of a document-denoising Restormer. We would have to
  export DocRes ourselves, host the `.onnx` files (as we do for UVDoc), and
  load the pickles with `weights_only=False`, which runs code, in a one-off
  sandboxed export step.
- **Runtime fit:** it would run on the ONNX Runtime that `oar-ocr` already
  ships. Under the `sheaf-image` boundary
  ([`todos/m8-sheaf-image.md`](../../todos/m8-sheaf-image.md)) a model can't
  live in `sheaf-image`. It would sit beside UVDoc, behind
  `sheaf_ocr`/`sheaf_ops` model lookup, and stay opt-in.

### 1.5 What it could mean for sheaf

The best candidates, in order:

1. **Deblurring phone photos.** sheaf has nothing for blur today, and this
   is DocRes's clearest win. Blurry captures are common with File > Create
   > From Scans/Photos….
2. **Binarizing degraded scans.** Faded carbon copies, thermal receipts and
   photocopies of photocopies are where Sauvola struggles. DocRes is within
   about 1.3 FM of the best binarizer, and it takes Sauvola's output as its
   prompt. Worth an A/B test on tax-form scans, measured by OCR accuracy as
   well as by eye.
3. **Appearance and deshadowing** as an opt-in alternative to background
   division, for photos where a hard shadow edge survives our filter.
4. **Dewarping:** probably not. UVDoc already covers it, and DocRes's
   dewarping needs a second model (the MBD mask) without beating the
   dedicated models.

Open questions before this becomes a roadmap item:
- CPU time per page at 1600 px and at full 300 dpi with tiling. 183 GFLOPs
  at an unknown size could mean several seconds per page.
- Whether restoring the image before OCR improves PP-OCRv5's and
  Tesseract's accuracy on our scans, or just looks nicer.
- The license on the weights.

## 2. Page parsing (layout, tables, Markdown)

These matter for the pending **Export to Markdown** item in
[ROADMAP.md](../../ROADMAP.md#pending). The `oar-ocr` 0.10 crate that
`sheaf-ocr` already depends on includes more than OCR (from its README):

- **`OARStructureBuilder`:** layout detection (PP-DocLayout S/M/L,
  PP-DocLayout Plus-L, PP-DocLayoutV2/V3, PP-DocBlockLayout), table
  classification, cell detection and structure recognition (SLANet,
  SLANet+, SLANeXt), and formula recognition. The result has
  `to_markdown()`. These are small ONNX models, the same kind we run today.
- **Vision-language page parsers** of 0.6–3 B parameters, which turn a
  whole page into Markdown or structured output: PaddleOCR-VL (0.9 B),
  GLM-OCR (0.9 B), MinerU2.5 (1.2 B), MonkeyOCRv2-S (0.6 B), OvisOCR2
  (0.8 B), jina-ocr-v1 (3 B MoE), HunyuanOCR (1 B) and others. These are a
  different weight class: gigabytes of weights, and likely too slow on CPU
  for a binder (unverified). They suit an opt-in or plugin-backed path
  more than the default.

So a Markdown export has a structured path ready in a dependency we already
have: our own text layer for born-digital pages, and `oar-ocr`'s structure
pipeline for scans. Neither path is tested on sheaf's documents yet.
