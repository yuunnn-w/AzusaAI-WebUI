/* ============================================================
   OfficeWrite —— 单文件内联 OOXML 生成 / 编辑核心(ES5:只用 var / function)

   定位(方案 shared/specs/office-tool-plan.md §2.3 / §2.8):
   · 只做**纯数据变换** —— 不碰工作区、不发请求、不写日志;工作区读写与权限
     全部留在 app 层的 officeToolRun。
   · 与 OfficeKit 不同:不建 worker,直接在主线程跑(写入是短任务,复用 worker
     要再传一份库体)。
   · ZIP 层取 fflate 用「CJS 垫片」:UMD 的第三分支是
     (typeof self != 'undefined' ? self : this).fflate = f(),浏览器里
     self === window ⇒ 直接用全局分支必然写脏 window.fflate。这里给
     module / exports 走 CJS 分支,window.fflate 全程不被触碰(方案 §3 S4 / A7 / V19)。
   · 结果契约:run() 永不 reject,失败返回 { ok:false, code, error }。
     内部各分层函数同款:成功 { ok:true, ... }、失败 { ok:false, code, error };
     **唯一例外**是无失败面的取值助手(xmlDeclOf / xmlText / bytesToText / textToBytes)。
   · 本片段的落地进度:**批次 A** = 载荷地基(ZIP 层 + XML 层 + 常量表);
     **批次 B** = docx 生成与编辑(S7–S12:create / outline / append /
     replace_text / delete / set_properties)+ validatePackage 的 ①–④;
     **批次 C** = pptx 生成与编辑(S13–S18:create / outline / set_text /
     add_slide / delete / replace_text / set_properties + 固定 16 件模板);
     **批次 F** = 图表 + 内嵌工作簿(S28–S32:add_chart 的 pptx / docx 两套宿主 +
     xlsxMinimal 手写工作簿 + chartSpace 生成器 + validatePackage 的 ⑤⑥⑦);
     **批次 G** = 页眉页脚(S34 docx / S35 pptx);**批次 H** = 表格 / 图片 / 套用版式
     (S37 add_table / S38 add_image / S39 apply_layout —— **S40 已取消**,
     方案 §14.4:`properties.page` 移出 v1)⇒ 13 项 operation 全部落地。
   · 与 src/officekit.src.js 同样以 text/plain 载荷(id=office-lib-*)为输入,
     零网络、零新库(fflate 已在 office.part 内)。
   ============================================================ */
(function (W) {
  "use strict";

  var VERSION = "fflate 0.8.3 + 原生 DOMParser/XMLSerializer";
  var FORMATS = ["docx", "pptx"];     /* xlsx 只作图表的内嵌工作簿被生成,不对外暴露 */
  var FFLATE_ID = "office-lib-fflate";

  /* ---------------- 命名空间常量表(方案 §3 S5) ---------------- */
  var NS = {
    /* OPC 包层 */
    ct: "http://schemas.openxmlformats.org/package/2006/content-types",
    pr: "http://schemas.openxmlformats.org/package/2006/relationships",
    /* 文档属性 */
    cp: "http://schemas.openxmlformats.org/package/2006/metadata/core-properties",
    dc: "http://purl.org/dc/elements/1.1/",
    dcterms: "http://purl.org/dc/terms/",
    dcmitype: "http://purl.org/dc/dcmitype/",
    xsi: "http://www.w3.org/2001/XMLSchema-instance",
    ep: "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties",
    vt: "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes",
    /* 关系(部件级 rels) */
    r: "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    /* WordprocessingML */
    w: "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    /* DrawingML(图表 / 表格 / 形状 / 图片共用) */
    a: "http://schemas.openxmlformats.org/drawingml/2006/main",
    c: "http://schemas.openxmlformats.org/drawingml/2006/chart",
    pic: "http://schemas.openxmlformats.org/drawingml/2006/picture",
    wp: "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    /* PresentationML */
    p: "http://schemas.openxmlformats.org/presentationml/2006/main",
    /* XML 规范自带的命名空间(批次 B:w:t 保首尾空格要写 xml:space="preserve") */
    xml: "http://www.w3.org/XML/1998/namespace"
  };

  /* ---------------- 部件名常量表(方案 §2.4) ---------------- */
  var PART = {
    contentTypes: "[Content_Types].xml",
    rootRels: "_rels/.rels",
    core: "docProps/core.xml",
    app: "docProps/app.xml",
    /* docx(固定 7 件 + 按需 word/_rels/document.xml.rels) */
    document: "word/document.xml",
    documentRels: "word/_rels/document.xml.rels",
    styles: "word/styles.xml",
    numbering: "word/numbering.xml",
    settings: "word/settings.xml",
    header1: "word/header1.xml",
    footer1: "word/footer1.xml",
    /* pptx(固定 16 件 + 每张幻灯 2 件) */
    presentation: "ppt/presentation.xml",
    presentationRels: "ppt/_rels/presentation.xml.rels",
    theme: "ppt/theme/theme1.xml",
    slideMaster: "ppt/slideMasters/slideMaster1.xml",
    slideMasterRels: "ppt/slideMasters/_rels/slideMaster1.xml.rels",
    layout1: "ppt/slideLayouts/slideLayout1.xml",
    layout2: "ppt/slideLayouts/slideLayout2.xml",
    presProps: "ppt/presProps.xml",
    viewProps: "ppt/viewProps.xml",
    tableStyles: "ppt/tableStyles.xml"
  };

  var MIME = {
    docx: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    pptx: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    xlsx: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    png: "image/png",
    jpeg: "image/jpeg"
  };

  /* ---------------- 容量限额(方案 §2.3 / §2.7) ----------------
     下面 5 个值与解析层 src/office-worker.src.js 的 LIMITS **必须逐值相等**
     (单一来源,改名不改值也算失配):
       本文件 maxEntries / maxTotalUncompressed / maxSingleEntry / maxRatio / maxImageBytes
       ↔ 解析层 maxEntries / maxUncompressedTotal / maxSingleEntry / maxRatio / maxImageBytes
     并由 maxImagesPerCall x maxImageBytes == 解析层 maxImagesTotal 这条算式锁在一起。
     构建期由 scripts/make-office-part.js 的常量等值断言机械核验(方案 §3 S2 第 ⑦ 点)——
     写成十进制字面量(不用 150 * 1024 * 1024 这类算式),断言才能正则取值。 */
  var LIMITS = {
    maxBytes: 20971520,               /* 输入文件 20 MB(整包解压 + 重压缩的内存峰值 ≈ 输入 3-8 倍) */
    maxSlides: 200,
    maxEntries: 2000,
    maxTotalUncompressed: 157286400,  /* 150 MB */
    maxSingleEntry: 67108864,         /* 64 MB */
    maxRatio: 120,
    maxImageBytes: 8388608,           /* 8 MB */
    maxImagesPerCall: 4,              /* 4 张 x 8 MB = 32 MB = 解析层 maxImagesTotal */
    maxTableRows: 200,
    maxTableCols: 30,
    maxChartSeries: 8,
    maxChartPoints: 64,
    maxChartsPerDoc: 20
  };

  /* ZIP 压缩方法白名单:0 = stored、8 = deflate;其余(OLE2 之外的 bzip2 / lzma /
     deflate64 / AES 等)一律拒收 —— fflate 也只会解这两种 */
  var METHOD_OK = { 0: true, 8: true };

  /* ---------------- XML 声明(方案 §3 S5;批次 A 审查 P3-2 裁定 (a)) ----------------
     模板部件一律以标准声明开头(与 docx / pptx 惯例一致);xmlSerialize 对"原件没有
     声明"的部件也会**合成**这一份,避免静默产出无声明部件。模板里必须逐件写成
     **字面量**(不能用下面这个常量拼)——make-office-part.js 的检测 13 逐件核。 */
  var XML_DECL_STD = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>';

  /* ============================================================
     docx 部件模板(方案 §2.4:固定 7 件 + 按需 1 件)
     书写纪律(方案 §2.4「模板书写纪律」,构建期检测 5 / 检测 13):
       · 不得出现 script 标签的字面量(开 / 闭两种形态都会破坏内联 script 块)
       · 避免出现 "<!--"(会被构建脚本转义、并进 note 列表)
       · 每件必须以 XML_DECL_STD 的字面量开头(检测 13 逐件断言)
       · 部件名一律不写死在这里 —— 落盘时按 PART.* 常量取(单一来源)
     体积账:约 9.3 KB(方案 §2.4 小计);其中 styles / numbering 是生成器的
     固定底稿,不随后续操作改写(编辑类 operation 只碰它点名的部件)。
     ============================================================ */
  var TPL_DOCX = {
    contentTypes: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<Types xmlns="' + 'http://schemas.openxmlformats.org/package/2006/content-types">\n'
      + '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>\n'
      + '<Default Extension="xml" ContentType="application/xml"/>\n'
      + '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>\n'
      + '<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>\n'
      + '<Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>\n'
      + '<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>\n'
      + '<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>\n'
      + '</Types>\n',
    rootRels: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<Relationships xmlns="' + 'http://schemas.openxmlformats.org/package/2006/relationships">\n'
      + '<Relationship Id="rId1" Type="' + 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>\n'
      + '<Relationship Id="rId2" Type="' + 'http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>\n'
      + '<Relationship Id="rId3" Type="' + 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>\n'
      + '</Relationships>\n',
    core: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<cp:coreProperties xmlns:cp="' + 'http://schemas.openxmlformats.org/package/2006/metadata/core-properties"'
      + ' xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/"'
      + ' xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">\n'
      + '<dc:title></dc:title>\n'
      + '<dc:subject></dc:subject>\n'
      + '<dc:creator></dc:creator>\n'
      + '<cp:keywords></cp:keywords>\n'
      + '<dc:description></dc:description>\n'
      + '<cp:lastModifiedBy></cp:lastModifiedBy>\n'
      + '<cp:revision>1</cp:revision>\n'
      + '<dcterms:created xsi:type="dcterms:W3CDTF">2026-01-01T00:00:00Z</dcterms:created>\n'
      + '<dcterms:modified xsi:type="dcterms:W3CDTF">2026-01-01T00:00:00Z</dcterms:modified>\n'
      + '</cp:coreProperties>\n',
    app: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<Properties xmlns="' + 'http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"'
      + ' xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">\n'
      + '<Application>AzusaAI WebUI</Application>\n'
      + '<DocSecurity>0</DocSecurity>\n'
      + '<ScaleCrop>false</ScaleCrop>\n'
      + '<Company></Company>\n'
      + '<LinksUpToDate>false</LinksUpToDate>\n'
      + '<SharedDoc>false</SharedDoc>\n'
      + '<HyperlinksChanged>false</HyperlinksChanged>\n'
      + '<AppVersion>1.0000</AppVersion>\n'
      + '<Paragraphs>1</Paragraphs>\n'
      + '</Properties>\n',
    document: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<w:document xmlns:w="' + 'http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
      + ' xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
      + ' xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"'
      + ' xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"'
      + ' xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">\n'
      + '<w:body>\n'
      + '<w:sectPr>\n'
      + '<w:pgSz w:w="11906" w:h="16838"/>\n'
      + '<w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="851" w:footer="992" w:gutter="0"/>\n'
      + '<w:cols w:space="425"/>\n'
      + '<w:docGrid w:linePitch="312"/>\n'
      + '</w:sectPr>\n'
      + '</w:body>\n'
      + '</w:document>\n',
    styles: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<w:styles xmlns:w="' + 'http://schemas.openxmlformats.org/wordprocessingml/2006/main">\n'
      + '<w:docDefaults>\n'
      + '<w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="SimSun" w:cs="Calibri"/><w:sz w:val="22"/><w:szCs w:val="22"/></w:rPr></w:rPrDefault>\n'
      + '<w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault>\n'
      + '</w:docDefaults>\n'
      + '<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>\n'
      + '<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:spacing w:after="300"/><w:contextualSpacing/></w:pPr><w:rPr><w:b/><w:sz w:val="56"/><w:szCs w:val="56"/></w:rPr></w:style>\n'
      + '<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/><w:szCs w:val="32"/></w:rPr></w:style>\n'
      + '<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="200" w:after="100"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="28"/><w:szCs w:val="28"/></w:rPr></w:style>\n'
      + '<w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="160" w:after="80"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr></w:style>\n'
      + '<w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:ind w:left="720"/><w:contextualSpacing/></w:pPr></w:style>\n'
      + '</w:styles>\n',
    numbering: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<w:numbering xmlns:w="' + 'http://schemas.openxmlformats.org/wordprocessingml/2006/main">\n'
      + '<w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="hybridMultilevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="&#8226;"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum>\n'
      + '<w:abstractNum w:abstractNumId="1"><w:multiLevelType w:val="hybridMultilevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum>\n'
      + '<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>\n'
      + '<w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>\n'
      + '</w:numbering>\n',
    /* 按需件:出现页眉 / 页脚 / 图片 / 图表 / 超链接这类"跨部件引用"时才落盘
       (方案 §2.4;纯文本 create 的产出不含它)。批次 B 只备模板不用它 ——
       批次 F 的 add_chart / 批次 G/H 的 add_image / set_header_footer 才会写入。 */
    documentRels: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<Relationships xmlns="' + 'http://schemas.openxmlformats.org/package/2006/relationships">\n'
      + '</Relationships>\n',
    /* 按需件(批次 G / S34):页眉 / 页脚 / settings —— 只有 set_header_footer 才落盘。
       形态逐条对齐真值件 gt-hdrftr.docx(Word COM 生成,方案 §16.2 P-G):
         根元素 = w:hdr / w:ftr(内容 = w:p > w:r > w:t;空文本 ⇒ 单个空 w:p)
         word/settings.xml = w:settings(w:evenAndOddHeaders 由"奇偶不同"开启时保留)
       部件名不写死在这里 —— 落盘时按 headerPart(n) / footerPart(n) / PART.settings 取 */
    header: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<w:hdr xmlns:w="' + 'http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
      + ' xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">\n'
      + '<w:p/>\n'
      + '</w:hdr>\n',
    footer: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<w:ftr xmlns:w="' + 'http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
      + ' xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">\n'
      + '<w:p/>\n'
      + '</w:ftr>\n',
    settings: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<w:settings xmlns:w="' + 'http://schemas.openxmlformats.org/wordprocessingml/2006/main">\n'
      + '<w:evenAndOddHeaders/>\n'
      + '</w:settings>\n'
  };

  /* docx 内容类型串(CT Override 用;set_properties 新建 core.xml 时要补) */
  var CT_DOCX = {
    core: 'application/vnd.openxmlformats-package.core-properties+xml',
    app: 'application/vnd.openxmlformats-officedocument.extended-properties+xml',
    document: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml',
    styles: 'application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml',
    numbering: 'application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml',
    /* 批次 G(S34):两条 CT Override 串 —— 逐字取自真值件 gt-hdrftr.docx(§16.2 P-G) */
    header: 'application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml',
    footer: 'application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml',
    settings: 'application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml'
  };
  /* 包级关系类型(_rels/.rels;set_properties 新建 core.xml 时要补) */
  var REL_CORE = 'http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties';
  var REL_OFFICE_DOC = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument';
  var REL_APP = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties';
  var REL_DOC_RELS = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/';
  /* 部件级关系类型(批次 G:S34 的页眉 / 页脚 / settings) */
  var REL_HEADER = REL_DOC_RELS + 'header';
  var REL_FOOTER = REL_DOC_RELS + 'footer';
  var REL_SETTINGS = REL_DOC_RELS + 'settings';

  /* ============================================================
     pptx 部件模板(方案 §2.4:固定 16 件 + 每张幻灯 2 件;§3 S13)
     固定 16 件 = [Content_Types].xml / _rels/.rels / docProps/{core,app}.xml /
       ppt/presentation.xml / ppt/_rels/presentation.xml.rels / ppt/theme/theme1.xml /
       ppt/slideMasters/slideMaster1.xml / ppt/slideMasters/_rels/slideMaster1.xml.rels /
       ppt/slideLayouts/slideLayout{1,2}.xml / ppt/slideLayouts/_rels/slideLayout{1,2}.xml.rels /
       ppt/{presProps,viewProps,tableStyles}.xml —— 即下面 15 键 + docProps/core.xml
       (**core 与 docx 同型,复用 TPL_DOCX.core**;避免两份同样的 cp:coreProperties 漂移)。
     "每张幻灯 2 件" = ppt/slides/slideN.xml + ppt/slides/_rels/slideN.xml.rels
       (slide 键是**每张幻灯的原型**,由 pptxSlideXml 逐张填充)。
     书写纪律(同 §2.4 / 构建期检测 5 与检测 13):
       · 不得出现 script 标签字面量(开 / 闭两形态);不写 XML 注释(会被转义 + 进 note)
       · 每件必须以 XML_DECL_STD 的**字面量**开头(检测 13 逐件断言)
       · 本块必须**扁平一层键**(检测 13 的键数 == 声明数判据);新增子对象请另起 var TPL_*
     形态口径(逐条来自真值实读,见 progress 的"现读取证"):
       · 两个版式 = 封面(ctrTitle + subTitle idx=1)+ 标题和内容(title + body idx=1)
       · 母版与两个版式**都含 dt/ftr/sldNum 三个占位符**(§13.3;母版 idx = 2/3/4、版式 idx = 10/11/12)
       · tableStyles 的 def 必须 = 批次 H 的 add_table 要引用的 GUID(§14.1;真值一致)
       · 16:9 = p:sldSz 12192000 x 6858000 EMU;notesSz = 6858000 x 9144000
     ============================================================ */
  var TPL_PPTX = {
    contentTypes: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">\n'
      + '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>\n'
      + '<Default Extension="xml" ContentType="application/xml"/>\n'
      + '<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>\n'
      + '<Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/>\n'
      + '<Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/>\n'
      + '<Override PartName="/ppt/slideLayouts/slideLayout2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/>\n'
      + '<Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>\n'
      + '<Override PartName="/ppt/presProps.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presProps+xml"/>\n'
      + '<Override PartName="/ppt/viewProps.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.viewProps+xml"/>\n'
      + '<Override PartName="/ppt/tableStyles.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml"/>\n'
      + '<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>\n'
      + '<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>\n'
      + '</Types>\n',
    rootRels: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">\n'
      + '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>\n'
      + '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>\n'
      + '<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>\n'
      + '</Relationships>\n',
    app: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"'
      + ' xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">\n'
      + '<Application>AzusaAI WebUI</Application>\n'
      + '<PresentationFormat>On-screen Show (16:9)</PresentationFormat>\n'
      + '<Paragraphs>0</Paragraphs>\n'
      + '<Slides>0</Slides>\n'
      + '<Notes>0</Notes>\n'
      + '<HiddenSlides>0</HiddenSlides>\n'
      + '<MMClips>0</MMClips>\n'
      + '<ScaleCrop>false</ScaleCrop>\n'
      + '<LinksUpToDate>false</LinksUpToDate>\n'
      + '<SharedDoc>false</SharedDoc>\n'
      + '<HyperlinksChanged>false</HyperlinksChanged>\n'
      + '<AppVersion>1.0000</AppVersion>\n'
      + '</Properties>\n',
    presentation: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"'
      + ' xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
      + ' xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" saveSubsetFonts="1">\n'
      + '<p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst>\n'
      + '<p:sldIdLst/>\n'
      + '<p:sldSz cx="12192000" cy="6858000"/>\n'
      + '<p:notesSz cx="6858000" cy="9144000"/>\n'
      + '<p:defaultTextStyle><a:defPPr><a:defRPr lang="zh-CN"/></a:defPPr>'
      + '<a:lvl1pPr marL="0" algn="l" defTabSz="914400" rtl="0" eaLnBrk="1" latinLnBrk="0" hangingPunct="1">'
      + '<a:defRPr sz="1800" kern="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill>'
      + '<a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/><a:cs typeface="+mn-cs"/></a:defRPr></a:lvl1pPr>'
      + '</p:defaultTextStyle>\n'
      + '</p:presentation>\n',
    presentationRels: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">\n'
      + '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>\n'
      + '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/>\n'
      + '<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/presProps" Target="presProps.xml"/>\n'
      + '<Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/viewProps" Target="viewProps.xml"/>\n'
      + '<Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/tableStyles" Target="tableStyles.xml"/>\n'
      + '</Relationships>\n',
    theme: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="AzusaAI">\n'
      + '<a:themeElements>\n'
      + '<a:clrScheme name="AzusaAI">\n'
      + '<a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>\n'
      + '<a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>\n'
      + '<a:dk2><a:srgbClr val="44546A"/></a:dk2>\n'
      + '<a:lt2><a:srgbClr val="E7E6E6"/></a:lt2>\n'
      + '<a:accent1><a:srgbClr val="4472C4"/></a:accent1>\n'
      + '<a:accent2><a:srgbClr val="ED7D31"/></a:accent2>\n'
      + '<a:accent3><a:srgbClr val="A5A5A5"/></a:accent3>\n'
      + '<a:accent4><a:srgbClr val="FFC000"/></a:accent4>\n'
      + '<a:accent5><a:srgbClr val="5B9BD5"/></a:accent5>\n'
      + '<a:accent6><a:srgbClr val="70AD47"/></a:accent6>\n'
      + '<a:hlink><a:srgbClr val="0563C1"/></a:hlink>\n'
      + '<a:folHlink><a:srgbClr val="954F72"/></a:folHlink>\n'
      + '</a:clrScheme>\n'
      + '<a:fontScheme name="AzusaAI">\n'
      + '<a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface="SimSun"/><a:cs typeface=""/></a:majorFont>\n'
      + '<a:minorFont><a:latin typeface="Calibri"/><a:ea typeface="SimSun"/><a:cs typeface=""/></a:minorFont>\n'
      + '</a:fontScheme>\n'
      + '<a:fmtScheme name="AzusaAI">\n'
      + '<a:fillStyleLst>\n'
      + '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>\n'
      + '<a:gradFill rotWithShape="1"><a:gsLst>'
      + '<a:gs pos="0"><a:schemeClr val="phClr"><a:lumMod val="110000"/><a:satMod val="105000"/><a:tint val="67000"/></a:schemeClr></a:gs>'
      + '<a:gs pos="100000"><a:schemeClr val="phClr"><a:lumMod val="105000"/><a:satMod val="103000"/><a:shade val="73000"/></a:schemeClr></a:gs>'
      + '</a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill>\n'
      + '<a:gradFill rotWithShape="1"><a:gsLst>'
      + '<a:gs pos="0"><a:schemeClr val="phClr"><a:satMod val="103000"/><a:lumMod val="102000"/><a:tint val="94000"/></a:schemeClr></a:gs>'
      + '<a:gs pos="100000"><a:schemeClr val="phClr"><a:satMod val="110000"/><a:lumMod val="100000"/><a:shade val="100000"/></a:schemeClr></a:gs>'
      + '</a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill>\n'
      + '</a:fillStyleLst>\n'
      + '<a:lnStyleLst>\n'
      + '<a:ln w="6350" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln>\n'
      + '<a:ln w="12700" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln>\n'
      + '<a:ln w="19050" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln>\n'
      + '</a:lnStyleLst>\n'
      + '<a:effectStyleLst>\n'
      + '<a:effectStyle><a:effectLst/></a:effectStyle>\n'
      + '<a:effectStyle><a:effectLst/></a:effectStyle>\n'
      + '<a:effectStyle><a:effectLst/></a:effectStyle>\n'
      + '</a:effectStyleLst>\n'
      + '<a:bgFillStyleLst>\n'
      + '<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>\n'
      + '<a:solidFill><a:schemeClr val="phClr"><a:tint val="95000"/><a:satMod val="170000"/></a:schemeClr></a:solidFill>\n'
      + '<a:gradFill rotWithShape="1"><a:gsLst>'
      + '<a:gs pos="0"><a:schemeClr val="phClr"><a:tint val="93000"/><a:satMod val="150000"/><a:shade val="98000"/><a:lumMod val="102000"/></a:schemeClr></a:gs>'
      + '<a:gs pos="100000"><a:schemeClr val="phClr"><a:tint val="98000"/><a:satMod val="130000"/><a:shade val="90000"/><a:lumMod val="103000"/></a:schemeClr></a:gs>'
      + '</a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill>\n'
      + '</a:bgFillStyleLst>\n'
      + '</a:fmtScheme>\n'
      + '</a:themeElements>\n'
      + '<a:objectDefaults/>\n'
      + '<a:extraClrSchemeLst/>\n'
      + '</a:theme>\n',
    slideMaster: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<p:sldMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"'
      + ' xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
      + ' xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">\n'
      + '<p:cSld>\n'
      + '<p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg>\n'
      + '<p:spTree>\n'
      + '<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\n'
      + '<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title Placeholder 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="838200" y="365125"/><a:ext cx="10515600" cy="1325563"/></a:xfrm>'
      + '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>'
      + '<p:txBody><a:bodyPr vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0" anchor="ctr"/><a:lstStyle/><a:p/></p:txBody></p:sp>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="3" name="Body Placeholder 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="838200" y="1825625"/><a:ext cx="10515600" cy="4351338"/></a:xfrm>'
      + '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>'
      + '<p:txBody><a:bodyPr vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0"/><a:lstStyle/><a:p/></p:txBody></p:sp>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="4" name="Date Placeholder 3"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="dt" sz="half" idx="2"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="838200" y="6356350"/><a:ext cx="2743200" cy="365125"/></a:xfrm>'
      + '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>'
      + '<p:txBody><a:bodyPr vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0" anchor="ctr"/>'
      + '<a:lstStyle><a:lvl1pPr algn="l"><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"><a:tint val="75000"/></a:schemeClr></a:solidFill></a:defRPr></a:lvl1pPr></a:lstStyle>'
      + '<a:p/></p:txBody></p:sp>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="5" name="Footer Placeholder 4"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="ftr" sz="quarter" idx="3"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="4038600" y="6356350"/><a:ext cx="4114800" cy="365125"/></a:xfrm>'
      + '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>'
      + '<p:txBody><a:bodyPr vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0" anchor="ctr"/>'
      + '<a:lstStyle><a:lvl1pPr algn="ctr"><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"><a:tint val="75000"/></a:schemeClr></a:solidFill></a:defRPr></a:lvl1pPr></a:lstStyle>'
      + '<a:p/></p:txBody></p:sp>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="6" name="Slide Number Placeholder 5"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="sldNum" sz="quarter" idx="4"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="8686800" y="6356350"/><a:ext cx="2743200" cy="365125"/></a:xfrm>'
      + '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>'
      + '<p:txBody><a:bodyPr vert="horz" lIns="91440" tIns="45720" rIns="91440" bIns="45720" rtlCol="0" anchor="ctr"/>'
      + '<a:lstStyle><a:lvl1pPr algn="r"><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"><a:tint val="75000"/></a:schemeClr></a:solidFill></a:defRPr></a:lvl1pPr></a:lstStyle>'
      + '<a:p/></p:txBody></p:sp>\n'
      + '</p:spTree>\n'
      + '</p:cSld>\n'
      + '<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3"'
      + ' accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>\n'
      + '<p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/><p:sldLayoutId id="2147483650" r:id="rId2"/></p:sldLayoutIdLst>\n'
      + '<p:txStyles>\n'
      + '<p:titleStyle><a:lvl1pPr algn="ctr" defTabSz="914400" rtl="0" eaLnBrk="1" latinLnBrk="0" hangingPunct="1">'
      + '<a:spcBef><a:spcPct val="0"/></a:spcBef><a:buNone/>'
      + '<a:defRPr sz="4400" kern="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill>'
      + '<a:latin typeface="+mj-lt"/><a:ea typeface="+mj-ea"/><a:cs typeface="+mj-cs"/></a:defRPr></a:lvl1pPr></p:titleStyle>\n'
      + '<p:bodyStyle><a:lvl1pPr marL="342900" indent="-342900" algn="l" defTabSz="914400" rtl="0" eaLnBrk="1" latinLnBrk="0" hangingPunct="1">'
      + '<a:spcBef><a:spcPct val="20000"/></a:spcBef><a:buFont typeface="Arial"/><a:buChar char="&#8226;"/>'
      + '<a:defRPr sz="2800" kern="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill>'
      + '<a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/><a:cs typeface="+mn-cs"/></a:defRPr></a:lvl1pPr></p:bodyStyle>\n'
      + '<p:otherStyle><a:defPPr><a:defRPr lang="zh-CN"/></a:defPPr>'
      + '<a:lvl1pPr marL="0" algn="l" defTabSz="914400" rtl="0" eaLnBrk="1" latinLnBrk="0" hangingPunct="1">'
      + '<a:defRPr sz="1800" kern="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill>'
      + '<a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/><a:cs typeface="+mn-cs"/></a:defRPr></a:lvl1pPr></p:otherStyle>\n'
      + '</p:txStyles>\n'
      + '</p:sldMaster>\n',
    slideMasterRels: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">\n'
      + '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>\n'
      + '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout2.xml"/>\n'
      + '<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/>\n'
      + '</Relationships>\n',
    layout1: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<p:sldLayout xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"'
      + ' xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
      + ' xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" type="title" preserve="1">\n'
      + '<p:cSld name="&#23553;&#38754;">\n'
      + '<p:spTree>\n'
      + '<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\n'
      + '<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="ctrTitle"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="1524000" y="1122363"/><a:ext cx="9144000" cy="2387600"/></a:xfrm></p:spPr>'
      + '<p:txBody><a:bodyPr anchor="b"/><a:lstStyle><a:lvl1pPr algn="ctr"><a:defRPr sz="6000"/></a:lvl1pPr></a:lstStyle><a:p/></p:txBody></p:sp>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="3" name="Subtitle 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="subTitle" idx="1"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="1524000" y="3602038"/><a:ext cx="9144000" cy="1655762"/></a:xfrm></p:spPr>'
      + '<p:txBody><a:bodyPr/><a:lstStyle><a:lvl1pPr algn="ctr"><a:defRPr sz="2400"/></a:lvl1pPr></a:lstStyle><a:p/></p:txBody></p:sp>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="4" name="Date Placeholder 3"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="dt" sz="half" idx="10"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="838200" y="6356350"/><a:ext cx="2743200" cy="365125"/></a:xfrm></p:spPr>'
      + '<p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle><a:lvl1pPr algn="l"><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"><a:tint val="75000"/></a:schemeClr></a:solidFill></a:defRPr></a:lvl1pPr></a:lstStyle><a:p/></p:txBody></p:sp>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="5" name="Footer Placeholder 4"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="ftr" sz="quarter" idx="11"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="4038600" y="6356350"/><a:ext cx="4114800" cy="365125"/></a:xfrm></p:spPr>'
      + '<p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle><a:lvl1pPr algn="ctr"><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"><a:tint val="75000"/></a:schemeClr></a:solidFill></a:defRPr></a:lvl1pPr></a:lstStyle><a:p/></p:txBody></p:sp>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="6" name="Slide Number Placeholder 5"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="sldNum" sz="quarter" idx="12"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="8686800" y="6356350"/><a:ext cx="2743200" cy="365125"/></a:xfrm></p:spPr>'
      + '<p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle><a:lvl1pPr algn="r"><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"><a:tint val="75000"/></a:schemeClr></a:solidFill></a:defRPr></a:lvl1pPr></a:lstStyle><a:p/></p:txBody></p:sp>\n'
      + '</p:spTree>\n'
      + '</p:cSld>\n'
      + '<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>\n'
      + '</p:sldLayout>\n',
    layout2: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<p:sldLayout xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"'
      + ' xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
      + ' xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" type="obj" preserve="1">\n'
      + '<p:cSld name="&#26631;&#39064;&#21644;&#20869;&#23481;">\n'
      + '<p:spTree>\n'
      + '<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\n'
      + '<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="838200" y="365125"/><a:ext cx="10515600" cy="1325563"/></a:xfrm></p:spPr>'
      + '<p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p/></p:txBody></p:sp>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="3" name="Content Placeholder 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="838200" y="1825625"/><a:ext cx="10515600" cy="4351338"/></a:xfrm></p:spPr>'
      + '<p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="4" name="Date Placeholder 3"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="dt" sz="half" idx="10"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="838200" y="6356350"/><a:ext cx="2743200" cy="365125"/></a:xfrm></p:spPr>'
      + '<p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle><a:lvl1pPr algn="l"><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"><a:tint val="75000"/></a:schemeClr></a:solidFill></a:defRPr></a:lvl1pPr></a:lstStyle><a:p/></p:txBody></p:sp>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="5" name="Footer Placeholder 4"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="ftr" sz="quarter" idx="11"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="4038600" y="6356350"/><a:ext cx="4114800" cy="365125"/></a:xfrm></p:spPr>'
      + '<p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle><a:lvl1pPr algn="ctr"><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"><a:tint val="75000"/></a:schemeClr></a:solidFill></a:defRPr></a:lvl1pPr></a:lstStyle><a:p/></p:txBody></p:sp>\n'
      + '<p:sp><p:nvSpPr><p:cNvPr id="6" name="Slide Number Placeholder 5"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>'
      + '<p:nvPr><p:ph type="sldNum" sz="quarter" idx="12"/></p:nvPr></p:nvSpPr>'
      + '<p:spPr><a:xfrm><a:off x="8686800" y="6356350"/><a:ext cx="2743200" cy="365125"/></a:xfrm></p:spPr>'
      + '<p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle><a:lvl1pPr algn="r"><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"><a:tint val="75000"/></a:schemeClr></a:solidFill></a:defRPr></a:lvl1pPr></a:lstStyle><a:p/></p:txBody></p:sp>\n'
      + '</p:spTree>\n'
      + '</p:cSld>\n'
      + '<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>\n'
      + '</p:sldLayout>\n',
    layout1Rels: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">\n'
      + '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/>\n'
      + '</Relationships>\n',
    layout2Rels: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">\n'
      + '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/>\n'
      + '</Relationships>\n',
    presProps: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<p:presentationPr xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"'
      + ' xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
      + ' xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"/>\n',
    viewProps: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<p:viewPr xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"'
      + ' xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
      + ' xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">\n'
      + '<p:normalViewPr><p:restoredLeft sz="15620"/><p:restoredTop sz="94660"/></p:normalViewPr>\n'
      + '<p:slideViewPr><p:cSldViewPr snapToGrid="0" snapToObjects="1">'
      + '<p:cViewPr varScale="1"><p:scale><a:sx n="100" d="100"/><a:sy n="100" d="100"/></p:scale>'
      + '<p:origin x="0" y="0"/></p:cViewPr><p:guideLst/></p:cSldViewPr></p:slideViewPr>\n'
      + '<p:notesTextViewPr><p:cViewPr><p:scale><a:sx n="100" d="100"/><a:sy n="100" d="100"/></p:scale>'
      + '<p:origin x="0" y="0"/></p:cViewPr></p:notesTextViewPr>\n'
      + '<p:gridSpacing cx="72008" cy="72008"/>\n'
      + '</p:viewPr>\n',
    tableStyles: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}"/>\n',
    slide: '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
      + '<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"'
      + ' xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
      + ' xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">\n'
      + '<p:cSld>\n'
      + '<p:spTree>\n'
      + '<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\n'
      + '<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>\n'
      + '</p:spTree>\n'
      + '</p:cSld>\n'
      + '<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>\n'
      + '</p:sld>\n'
  };

  /* pptx 内容类型串 / 关系类型串 / 部件名助手(单一来源:模板与编辑类 operation 共用) */
  var CT_PPTX = {
    core: CT_DOCX.core,
    app: CT_DOCX.app,
    presentation: 'application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml',
    slideMaster: 'application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml',
    slideLayout: 'application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml',
    slide: 'application/vnd.openxmlformats-officedocument.presentationml.slide+xml',
    theme: 'application/vnd.openxmlformats-officedocument.theme+xml'
  };
  var REL_SLIDE = REL_DOC_RELS + 'slide';
  var REL_SLIDE_LAYOUT = REL_DOC_RELS + 'slideLayout';
  var REL_SLIDE_MASTER = REL_DOC_RELS + 'slideMaster';
  var REL_THEME = REL_DOC_RELS + 'theme';
  var REL_PRES_PROPS = REL_DOC_RELS + 'presProps';
  var REL_VIEW_PROPS = REL_DOC_RELS + 'viewProps';
  var REL_TABLE_STYLES = REL_DOC_RELS + 'tableStyles';
  /* 每张幻灯的 2 件(方案 §2.4);rels 名**必须**由 relsPathOf(部件名) 派生 ——
     §2.5 单一命名源:写侧(create / add_slide)与读侧(pptxSlideList.relsPart /
     relsMapOf)只能有一个算法,否则产出 OPC 非法名、真实 PowerPoint 打不开 */
  function slidePart(n) { return 'ppt/slides/slide' + n + '.xml'; }
  function slideRelsPart(n) { return relsPathOf(slidePart(n)); }
  var LAYOUT1_RELS = 'ppt/slideLayouts/_rels/slideLayout1.xml.rels';
  var LAYOUT2_RELS = 'ppt/slideLayouts/_rels/slideLayout2.xml.rels';
  var PPTX_LAYOUT_KEYS = {
    title: { part: PART.layout1, rels: LAYOUT1_RELS, relKey: 'layout1Rels', target: '../slideLayouts/slideLayout1.xml' },
    content: { part: PART.layout2, rels: LAYOUT2_RELS, relKey: 'layout2Rels', target: '../slideLayouts/slideLayout2.xml' },
    blank: { part: PART.layout2, rels: LAYOUT2_RELS, relKey: 'layout2Rels', target: '../slideLayouts/slideLayout2.xml' }
  };

  /* ---------------- 通用助手 ---------------- */
  function has(o, k) { return Object.prototype.hasOwnProperty.call(o, k); }
  function fail(code, error) { return { ok: false, code: code, error: String(error) }; }
  function errOf(code, message) {
    var e = new Error(String(message));
    e.officeCode = code;
    return e;
  }
  function codeOf(e) { return (e && e.officeCode) ? e.officeCode : "EINTERNAL"; }
  function msgOf(e) {
    if (e == null) { return "未知错误"; }
    if (typeof e === "string") { return e; }
    return e.message ? String(e.message) : String(e);
  }
  function short(s) {
    var t = String(s == null ? "" : s).replace(/\s+/g, " ").replace(/^ | $/g, "");
    return t.length > 120 ? (t.slice(0, 120) + "…") : t;
  }
  function u16(u8, i) { return u8[i] | (u8[i + 1] << 8); }
  function u32(u8, i) {
    return (u8[i] | (u8[i + 1] << 8) | (u8[i + 2] << 16) | (u8[i + 3] << 24)) >>> 0;
  }
  /* ZIP64 的 64 位字段按 lo + hi * 2^32 读(包体积远不到 2^53,不做 BigInt) */
  function u64(u8, i) { return u32(u8, i) + u32(u8, i + 4) * 4294967296; }
  function startsWithBytes(u8, bytes) {
    if (u8.length < bytes.length) { return false; }
    for (var i = 0; i < bytes.length; i++) { if (u8[i] !== bytes[i]) { return false; } }
    return true;
  }
  var _dec = null, _enc = null;
  function decoder() { if (!_dec) { _dec = new W.TextDecoder("utf-8"); } return _dec; }
  function encoder() { if (!_enc) { _enc = new W.TextEncoder(); } return _enc; }
  /* 注意:TextDecoder 默认吃掉 BOM ⇒ 走文本往返的部件会丢 BOM(未点名的条目
     走的是字节复制,不受影响;相关边界见批次 A 报告) */
  function bytesToText(u8) { return decoder().decode(u8); }
  function textToBytes(s) { return encoder().encode(String(s)); }

  /* ---------------- 载荷读取(与 OfficeKit 的 unescapeInline 口径一致) ---------------- */
  var texts = null;
  /* 构建期 escapeForInline 的逆操作(make-office-part.js 断言逐字节可逆);
     这里用 "<" + "/script" 拼出结果,避免源内出现会终止 script 块的字面序列 */
  function unescapeInline(code) {
    return String(code).replace(/<\\\/script/gi, "<" + "/script").replace(/<\\!--/g, "<" + "!--");
  }
  function payloadText(id) {
    if (!texts) { texts = {}; }
    if (typeof texts[id] === "string") { return texts[id]; }
    var t = "";
    try {
      var el = W.document ? W.document.getElementById(id) : null;
      t = el ? unescapeInline(el.textContent) : "";
    } catch (e) { t = ""; }
    texts[id] = t;
    return t;
  }

  /* ---------------- fflate 取值:CJS 垫片(唯一不污染全局的取法) ----------------
     ✗ 禁止 new Function(src + ";return fflate;")() 及任何依赖 UMD 第三分支的写法 */
  var FF = null;
  function fflateOf() {
    if (FF) { return FF; }
    var src = payloadText(FFLATE_ID);
    if (!src) {
      throw errOf("EOFFICE", "缺少内嵌载荷 " + FFLATE_ID
        + "(请跑 node scripts/make-office-part.js 重新生成 src/office.part)");
    }
    var m = { exports: {} };
    try {
      /* eslint-disable-next-line no-new-func */
      new Function("module", "exports", src + "\nreturn module.exports;")(m, m.exports);
    } catch (e) {
      throw errOf("EOFFICE", "内嵌 fflate 载荷无法编译:" + msgOf(e));
    }
    var ff = m.exports;
    if (!ff || typeof ff.zipSync !== "function" || typeof ff.unzipSync !== "function"
      || typeof ff.strToU8 !== "function" || typeof ff.strFromU8 !== "function") {
      throw errOf("EOFFICE", "内嵌 fflate 载荷导出异常(缺 zipSync / unzipSync / strToU8 / strFromU8)");
    }
    FF = ff;
    return FF;
  }

  /* ---------------- ZIP 层:中央目录预扫描(零解压) ----------------
     判据顺序与解析层一致(首见即拒),阈值见 LIMITS(与解析层同值):
       加密标志 → ENOTSUP;未知压缩方法 → EBADZIP;条目数 / 单条解压 / 解压总量 /
       压缩比 超限 → E2BIG;结构损坏 → EBADZIP。
     先于任何解压执行(方案 §2.7「ZIP 炸弹 / 超限包」)。 */
  function findEocd(u8) {
    var min = u8.length - 22 - 65535;
    if (min < 0) { min = 0; }
    for (var i = u8.length - 22; i >= min; i--) {
      if (u8[i] === 0x50 && u8[i + 1] === 0x4B && u8[i + 2] === 0x05 && u8[i + 3] === 0x06) { return i; }
    }
    return -1;
  }
  function zipPrescan(u8) {
    if (!u8 || typeof u8.length !== "number" || u8.length < 22) {
      return fail("EBADZIP", "不是有效的 OOXML 包:内容为空或过短");
    }
    /* 加密的 OOXML / 老二进制格式(.doc / .ppt)都会被另存成 CFB(OLE2)容器 —— 靠魔数先认出来 */
    if (startsWithBytes(u8, [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1])) {
      return fail("ENOTSUP", "已加密或受口令保护(或为老二进制格式 .doc/.ppt),无法改写");
    }
    if (!(u8[0] === 0x50 && u8[1] === 0x4B)) {
      return fail("EBADZIP", "不是有效的 OOXML 包(缺少 ZIP 头)");
    }
    var at = findEocd(u8);
    if (at < 0) { return fail("EBADZIP", "不是有效的 OOXML 包:找不到 ZIP 中央目录结尾记录(EOCD)"); }
    var total = u16(u8, at + 10);
    var cdSize = u32(u8, at + 12);
    var cdOff = u32(u8, at + 16);
    var cmtLen = u16(u8, at + 20);
    var comment = "";
    try {
      comment = bytesToText(u8.subarray(at + 22, Math.min(at + 22 + cmtLen, u8.length)));
    } catch (e) { comment = ""; }
    var zip64 = false;
    if (total === 0xFFFF || cdSize === 0xFFFFFFFF || cdOff === 0xFFFFFFFF) {
      var loc = at - 20;
      if (loc >= 0 && u8[loc] === 0x50 && u8[loc + 1] === 0x4B && u8[loc + 2] === 0x06 && u8[loc + 3] === 0x07) {
        var zAt = u64(u8, loc + 8);
        if (zAt + 56 <= u8.length && u8[zAt] === 0x50 && u8[zAt + 1] === 0x4B
          && u8[zAt + 2] === 0x06 && u8[zAt + 3] === 0x06) {
          total = u64(u8, zAt + 32);
          cdSize = u64(u8, zAt + 40);
          cdOff = u64(u8, zAt + 48);
          zip64 = true;
        }
      }
      if (!zip64) { return fail("EBADZIP", "ZIP64 结构不完整(中央目录结尾记录不可读)"); }
    }
    if (cdOff + 46 > u8.length) { return fail("EBADZIP", "ZIP 中央目录偏移越界(文件可能被截断)"); }
    var p = cdOff, count = 0, totalOut = 0, ratioPeak = 0, ratioName = "";
    var order = [];
    while (count < total || (cdSize > 0 && p < cdOff + cdSize)) {
      if (p + 46 > u8.length) { return fail("EBADZIP", "ZIP 中央目录被截断(条目 " + count + " 处)"); }
      if (!(u8[p] === 0x50 && u8[p + 1] === 0x4B && u8[p + 2] === 0x01 && u8[p + 3] === 0x02)) {
        return fail("EBADZIP", "ZIP 中央目录签名异常(条目 " + count + " 处)");
      }
      var flags = u16(u8, p + 8);
      var method = u16(u8, p + 10);
      var compSize = u32(u8, p + 20);
      var uncompSize = u32(u8, p + 24);
      var nameLen = u16(u8, p + 28);
      var extraLen = u16(u8, p + 30);
      var cLen = u16(u8, p + 32);
      if (uncompSize === 0xFFFFFFFF || compSize === 0xFFFFFFFF) {
        return fail("E2BIG", "条目 " + count + " 使用了 ZIP64 单条长度字段(超出可改写上限)");
      }
      var name = "";
      try { name = bytesToText(u8.subarray(p + 46, p + 46 + nameLen)); } catch (e2) { name = ""; }
      if ((flags & 0x0001) || (flags & 0x0040)) {
        return fail("ENOTSUP", "已加密或受口令保护,无法改写(条目 " + short(name) + ")");
      }
      if (!METHOD_OK[method]) {
        return fail("EBADZIP", "压缩方法不在白名单(条目 " + short(name) + " 用方法 " + method + ")");
      }
      if (count + 1 > LIMITS.maxEntries) {
        return fail("E2BIG", "条目数超过 " + LIMITS.maxEntries + "(这条包有 " + (count + 1) + " 条起)");
      }
      if (uncompSize > LIMITS.maxSingleEntry) {
        return fail("E2BIG", "单个条目超过 " + Math.round(LIMITS.maxSingleEntry / 1048576) + "MB(条目 "
          + short(name) + ")");
      }
      totalOut += uncompSize;
      if (totalOut > LIMITS.maxTotalUncompressed) {
        return fail("E2BIG", "解压总量超过 " + Math.round(LIMITS.maxTotalUncompressed / 1048576) + "MB");
      }
      var ratio = compSize > 0 ? uncompSize / compSize : (uncompSize > 0 ? 1e9 : 1);
      if (ratio > ratioPeak) { ratioPeak = ratio; ratioName = name; }
      if (ratio > LIMITS.maxRatio) {
        return fail("E2BIG", "压缩比 " + Math.round(ratio) + ":1 超过 " + LIMITS.maxRatio + ":1(条目 "
          + short(name) + ")");
      }
      order.push(name);
      count++;
      p += 46 + nameLen + extraLen + cLen;
    }
    if (count !== total) {
      return fail("EBADZIP", "中央目录条数与 EOCD 声明不符(实际 " + count + ",声明 " + total + ")");
    }
    return {
      ok: true, entries: count, totalOut: totalOut,
      maxRatio: Math.round(ratioPeak * 10) / 10, maxRatioName: ratioName,
      order: order, meta: { zip64: zip64, comment: comment }
    };
  }

  /* 解包:filter(name) 返回真才解压;返回 { name2bytes, order }(order 取中央目录原序) */
  function zipUnzip(u8, filter) {
    var pre = zipPrescan(u8);
    if (!pre.ok) { return pre; }
    var ff;
    try { ff = fflateOf(); } catch (e) { return fail(codeOf(e), msgOf(e)); }
    var name2bytes = {};
    try {
      name2bytes = ff.unzipSync(u8, {
        filter: function (f) { return filter ? !!filter(f.name, f) : true; }
      });
    } catch (e2) {
      return fail("EBADZIP", "解压失败:" + msgOf(e2));
    }
    var order = [], i;
    for (i = 0; i < pre.order.length; i++) {
      if (has(name2bytes, pre.order[i])) { order.push(pre.order[i]); }
    }
    return { ok: true, name2bytes: name2bytes, order: order, meta: pre.meta, entries: pre.entries };
  }

  /* 打包:entries = { 部件名: Uint8Array|String };order 给定时按其顺序输出,
     其余按 entries 键序追加,`[Content_Types].xml` 恒排首位(OPC 惯例)。
     注:zipSync 未传 mtime ⇒ 时间戳取 Date.now(),同一输入两次产出字节不同 ——
     判据一律用「逐条目内容 md5」,不用整包 md5(方案 §2.7 明确不保项 ④)。 */
  function zipBuild(entries, order) {
    var ff;
    try { ff = fflateOf(); } catch (e) { return fail(codeOf(e), msgOf(e)); }
    var names = [], k;
    for (k in entries) { if (has(entries, k)) { names.push(k); } }
    if (!names.length) { return fail("EINVAL", "打包内容为空"); }
    var seq = [], i;
    if (order && order.length) {
      for (i = 0; i < order.length; i++) { if (has(entries, order[i])) { seq.push(order[i]); } }
    }
    for (i = 0; i < names.length; i++) { if (seq.indexOf(names[i]) < 0) { seq.push(names[i]); } }
    var files = {};
    for (i = 0; i < seq.length; i++) {
      var v = entries[seq[i]];
      if (typeof v === "string") { files[seq[i]] = ff.strToU8(v); }
      else if (v && typeof v.length === "number") { files[seq[i]] = v; }
      else { return fail("EINVAL", "条目 " + seq[i] + " 的内容不是字节或字符串"); }
    }
    var ct = PART.contentTypes;
    var ci = seq.indexOf(ct);
    if (ci > 0) {
      seq.splice(ci, 1);
      seq.unshift(ct);
      var reordered = {};
      reordered[ct] = files[ct];
      for (i = 0; i < seq.length; i++) { if (seq[i] !== ct) { reordered[seq[i]] = files[seq[i]]; } }
      files = reordered;
    }
    var bytes;
    try { bytes = ff.zipSync(files, { level: 6 }); } catch (e2) { return fail("EINTERNAL", "打包失败:" + msgOf(e2)); }
    return { ok: true, bytes: bytes, order: seq };
  }

  /* ---------------- XML 层(方案 §3 S5) ---------------- */
  function xmlParse(text) {
    var s = String(text == null ? "" : text);
    var doc = null;
    try {
      /* 走 W.DOMParser(lint 的「调用了但未声明」清单不含 DOMParser,借 window 取用) */
      doc = new W.DOMParser().parseFromString(s, "text/xml");
    } catch (e) {
      return fail("EBADZIP", "XML 解析失败:" + msgOf(e));
    }
    if (!doc || !doc.documentElement) { return fail("EBADZIP", "XML 解析失败:文档为空"); }
    var root = doc.documentElement;
    if (String(root.nodeName).toLowerCase() === "parsererror") {
      return fail("EBADZIP", "XML 解析失败(parsererror):" + short(root.textContent));
    }
    if (doc.getElementsByTagNameNS) {
      var pe = doc.getElementsByTagNameNS("*", "parsererror");
      if (pe && pe.length) {
        return fail("EBADZIP", "XML 解析失败(parsererror):" + short(pe[0].textContent));
      }
    }
    return { ok: true, doc: doc };
  }
  /* 原样捕获原文件的 XML 声明与它后面的空白(序列化时前置);有无声明都要能判 */
  function xmlDeclOf(text) {
    var m = /^(\s*)(<\?xml[^>]*\?>)(\s*)/.exec(String(text == null ? "" : text));
    if (!m) { return { decl: "", gap: "", hasDecl: false, standalone: false }; }
    return {
      decl: m[2], gap: m[3], hasDecl: true,
      standalone: /standalone\s*=\s*("yes"|'yes')/.test(m[2])
    };
  }
  /* decl 可传 xmlDeclOf 的返回对象、也可传声明字符串;缺省(或原件本就无声明)
     则**合成** XML_DECL_STD —— 见下面的 P3-2 裁定 */
  function xmlSerialize(doc, decl) {
    var out = "";
    try {
      out = new W.XMLSerializer().serializeToString(doc);
    } catch (e) {
      return fail("EINTERNAL", "XML 序列化失败:" + msgOf(e));
    }
    /* 序列化器**自己会**带声明(实测:serializeToString(Document) 以
       <?xml …?> 开头,serializeToString(documentElement) 则不带)—— 这里统一剥掉,
       只保留"原样捕获的那一份",否则声明会被写两遍 ⇒ 产物成非法 XML(批次 A S6 ③ 实抓) */
    out = out.replace(/^\s*<\?xml[^>]*\?>/, "");
    var d = "", gap = "";
    if (typeof decl === "string") { d = decl; }
    else if (decl) { d = decl.decl || ""; gap = decl.gap || ""; }
    /* 原件没有声明(或调用方没给)时**合成**标准声明:否则模板串任一件漏写声明就会
       **静默**产出无声明部件(方案 §3 S5;批次 A 审查 P3-2 裁定 (a))。
       这条与 make-office-part.js 的检测 13(模板必含声明)是同一个保障的两半。 */
    if (!d) { d = XML_DECL_STD; gap = ""; }
    return { ok: true, text: d + gap + out };
  }
  /* 深遍历取全部文本(注释 / PI 不算) */
  function xmlText(node) {
    if (!node) { return ""; }
    if (node.nodeType === 3 || node.nodeType === 4) { return node.nodeValue == null ? "" : String(node.nodeValue); }
    var out = "";
    var kids = node.childNodes || [];
    for (var i = 0; i < kids.length; i++) {
      var n = kids[i];
      if (n.nodeType === 3 || n.nodeType === 4) { out += n.nodeValue == null ? "" : String(n.nodeValue); }
      else if (n.nodeType === 1) { out += xmlText(n); }
    }
    return out;
  }

  /* ============================================================
     批次 B:docx 生成与编辑(方案 §3 的 S7–S12)
     分层:模板件装配(S7)→ 正文块构造(S8)→ 索引(S9)→ 追加(S10)→
           两阶段替换(S11)→ 删段 / 写属性(S12)→ 包结构自检(§12.8 ①–④)
     DOM 构造三陷阱(方案 §3 S8,写进实现):
       ① 新增节点一律用**该模板部件自己的 doc**.createElementNS(跨文档插入依赖
          浏览器自动 adopt,行为不一致);
       ② 只构造 DOM、不拼 XML 字符串(避免转义错误与注入);
       ③ 文本一律 createTextNode;需要保首尾 / 连续空格时给 w:t 加 xml:space="preserve"。
     ============================================================ */

  /* ---------------- DOM 构造助手 ---------------- */
  function elNs(doc, ns, qname) { return doc.createElementNS(ns, qname); }
  function elW(doc, name) { return doc.createElementNS(NS.w, "w:" + name); }
  function attrNS(el, ns, qname, val) { el.setAttributeNS(ns, qname, String(val)); return el; }
  function wAttr(el, name, val) { return attrNS(el, NS.w, "w:" + name, val); }
  function isSpaceSensitive(s) { return /^\s|\s$/.test(s) || /\s\s/.test(s); }
  /* 元素纯文本:清空后挂单个文本节点(需要保空格时写 xml:space) */
  function setElemText(doc, el, s) {
    var t = String(s == null ? "" : s);
    while (el.firstChild) { el.removeChild(el.firstChild); }
    if (isSpaceSensitive(t)) { el.setAttributeNS(NS.xml, "xml:space", "preserve"); }
    el.appendChild(doc.createTextNode(t));
    return el;
  }
  function wText(doc, s) { return setElemText(doc, elW(doc, "t"), s); }
  function wRun(doc, text, bold, italic) {
    var r = elW(doc, "r");
    if (bold || italic) {
      var rPr = elW(doc, "rPr");
      if (bold) { rPr.appendChild(elW(doc, "b")); }
      if (italic) { rPr.appendChild(elW(doc, "i")); }
      r.appendChild(rPr);
    }
    r.appendChild(wText(doc, text));
    return r;
  }
  function trimEnd(s) { return String(s == null ? "" : s).replace(/[ \t]+$/, ""); }
  /* 预览截断(方案 §2.8:段落 ≤80 字符);表头 / 页脚这类单值字段不截断。
     空白口径(批 C 修正):只把换行折成单个空格,**不压缩连续空格**(有意双空格
     是内容);keepNl=true(pptx outline)时连换行也保留,段落边界在返回串里以
     "\n" 可辨 —— outline→set_text 的索引往返因此无损 */
  function brief(s, n, keepNl) {
    var t = String(s == null ? "" : s).replace(/\r\n?/g, "\n");
    if (!keepNl) { t = t.replace(/\n+/g, " "); }
    t = t.replace(/^[ \n]+|[ \n]+$/g, "");
    return t.length > n ? (t.slice(0, n) + "…") : t;
  }
  function occCount(s, needle) { return String(s).split(needle).length - 1; }
  function isoNow() {
    try { return new Date().toISOString().replace(/\.[0-9]{3}Z$/, "Z"); }
    catch (e) { return "2026-01-01T00:00:00Z"; }
  }
  function progress(opts, phase) {
    var f = (opts && typeof opts.onProgress === "function") ? opts.onProgress : null;
    if (!f) { return; }
    try { f(String(phase)); } catch (e) { /* 进度回调抛错不影响写入 */ }
  }
  function stopIfAborted(opts) {
    return (opts && opts.signal && opts.signal.aborted) ? fail("EABORT", "操作已取消") : null;
  }

  /* ---------------- 行内与块解析(create / append 共用) ---------------- */
  /* 行内:`**粗**` / `*斜*` → run 描述表(不嵌套,与 inputSchema 的 description 一致) */
  function parseInline(src) {
    var s = String(src == null ? "" : src);
    var re = /\*\*([^*]+)\*\*|\*([^*]+)\*/g, out = [], m, last = 0;
    while ((m = re.exec(s)) !== null) {
      if (m.index > last) { out.push({ text: s.slice(last, m.index), b: false, i: false }); }
      if (m[1] != null) { out.push({ text: m[1], b: true, i: false }); }
      else { out.push({ text: m[2], b: false, i: true }); }
      last = m.index + m[0].length;
    }
    if (last < s.length) { out.push({ text: s.slice(last), b: false, i: false }); }
    if (!out.length) { out.push({ text: "", b: false, i: false }); }
    return out;
  }
  /* 块解析:`#`/`##`/`###` 标题、`- ` 或 `* ` 无序、`1. ` 或 `1) ` 有序,其余一行一段;
     空行跳过(不产出空段)。 */
  function parseContentLite(text) {
    var lines = String(text == null ? "" : text).replace(/\r\n?/g, "\n").split("\n");
    var blocks = [], i, m;
    for (i = 0; i < lines.length; i++) {
      var line = lines[i];
      if (!/\S/.test(line)) { continue; }
      m = /^ {0,3}(#{1,3})[ \t]+(\S.*)$/.exec(line);
      if (m) { blocks.push({ kind: "h" + m[1].length, text: trimEnd(m[2]) }); continue; }
      m = /^[ \t]*[-*][ \t]+(\S.*)$/.exec(line);
      if (m) { blocks.push({ kind: "ul", text: trimEnd(m[1]) }); continue; }
      m = /^[ \t]*[0-9]{1,9}[.)][ \t]+(\S.*)$/.exec(line);
      if (m) { blocks.push({ kind: "ol", text: trimEnd(m[1]) }); continue; }
      blocks.push({ kind: "p", text: trimEnd(line) });
    }
    return blocks;
  }
  var DOCX_STYLE_OF = { h1: "Heading1", h2: "Heading2", h3: "Heading3" };
  var DOCX_NUMID_OF = { ul: "1", ol: "2" };    /* 模板 numbering.xml 的两个 numId */
  function wParagraph(doc, block) {
    var p = elW(doc, "p");
    var numId = DOCX_NUMID_OF[block.kind] || "";
    var style = DOCX_STYLE_OF[block.kind] || (numId ? "ListParagraph" : "");
    if (style || numId) {
      var pPr = elW(doc, "pPr");
      if (style) { pPr.appendChild(wAttr(elW(doc, "pStyle"), "val", style)); }
      if (numId) {
        var numPr = elW(doc, "numPr");
        numPr.appendChild(wAttr(elW(doc, "ilvl"), "val", "0"));
        numPr.appendChild(wAttr(elW(doc, "numId"), "val", numId));
        pPr.appendChild(numPr);
      }
      p.appendChild(pPr);
    }
    var runs = parseInline(block.text), i;
    for (i = 0; i < runs.length; i++) { p.appendChild(wRun(doc, runs[i].text, runs[i].b, runs[i].i)); }
    return p;
  }

  /* ---------------- docx 定位与读值 ---------------- */
  function docxBody(doc) {
    var b = doc.getElementsByTagNameNS(NS.w, "body");
    return (b && b.length) ? b[0] : null;
  }
  /* body 级节属性(w:body 的直接子元素;docx 规定它必须留在 body 末尾) */
  function docxBodySectPr(body) {
    var kids = body.childNodes;
    for (var i = kids.length - 1; i >= 0; i--) {
      var n = kids[i];
      if (n.nodeType === 1 && n.namespaceURI === NS.w && n.localName === "sectPr") { return n; }
    }
    return null;
  }
  /* 正文项 = w:body 的直接子元素去掉 body 级 w:sectPr;数组下标即**内容序号**(0 起),
     outline 的 i / delete 的 paragraph_index 都用它 */
  function docxItems(doc) {
    var body = docxBody(doc);
    if (!body) { return []; }
    var kids = body.childNodes, out = [], i;
    for (i = 0; i < kids.length; i++) {
      var n = kids[i];
      if (!n || n.nodeType !== 1 || n.namespaceURI !== NS.w) { continue; }
      if (n.localName === "sectPr") { continue; }
      out.push(n);
    }
    return out;
  }
  /* 取文本口径:只取 w:t(不含域指令 w:instrText、不含公式 m:t) */
  function wTextsOf(node) {
    var ts = node.getElementsByTagNameNS(NS.w, "t"), out = "", i;
    for (i = 0; i < ts.length; i++) { out += xmlText(ts[i]); }
    return out;
  }
  function docxPStyle(p) {
    var kids = p.childNodes, i, j;
    for (i = 0; i < kids.length; i++) {
      var pr = kids[i];
      if (pr.nodeType !== 1 || pr.namespaceURI !== NS.w || pr.localName !== "pPr") { continue; }
      var ps = pr.childNodes;
      for (j = 0; j < ps.length; j++) {
        var s = ps[j];
        if (s.nodeType === 1 && s.namespaceURI === NS.w && s.localName === "pStyle") {
          return String(s.getAttributeNS(NS.w, "val") || "");
        }
      }
    }
    return "";
  }
  function docxTablePreview(tbl) {
    var rows = tbl.getElementsByTagNameNS(NS.w, "tr"), out = [], i, j;
    for (i = 0; i < rows.length && i < 3; i++) {
      var cells = rows[i].getElementsByTagNameNS(NS.w, "tc"), row = [];
      for (j = 0; j < cells.length; j++) { row.push(wTextsOf(cells[j])); }
      out.push(row.join(" | "));
    }
    return out.join(" / ");
  }
  function intAttr(el, name) {
    var v = el.getAttributeNS(NS.w, name);
    return (v && /^-?[0-9]+$/.test(v)) ? parseInt(v, 10) : 0;
  }
  var DOCX_PAGE_SIZES = [
    { w: 11906, h: 16838, name: "A4" },       /* 210 x 297 mm */
    { w: 12240, h: 15840, name: "Letter" },   /* 8.5 x 11 in */
    { w: 12240, h: 20160, name: "Legal" },
    { w: 16838, h: 23811, name: "A3" },
    { w: 8390, h: 11906, name: "A5" }
  ];
  /* 页面尺寸 / 方向:末节的 w:pgSz(**只读展示**;方案 v1.3 已把 set_properties.page
     移出 v1,§14.4 —— 这里只回读,不提供任何写入口) */
  function docxPageOf(doc) {
    var out = { size: null, orientation: null, width_twips: 0, height_twips: 0 };
    var body = docxBody(doc);
    var sectPr = body ? docxBodySectPr(body) : null;
    if (!sectPr) { return out; }
    var sizes = sectPr.getElementsByTagNameNS(NS.w, "pgSz");
    if (!sizes || !sizes.length) { return out; }
    var w = intAttr(sizes[0], "w"), h = intAttr(sizes[0], "h");
    var orient = String(sizes[0].getAttributeNS(NS.w, "orient") || "");
    out.width_twips = w;
    out.height_twips = h;
    out.orientation = orient || (w && h ? (w > h ? "landscape" : "portrait") : null);
    out.size = docxPageName(w && h ? Math.min(w, h) : 0, w && h ? Math.max(w, h) : 0);
    return out;
  }
  function docxPageName(sw, sh) {
    for (var i = 0; i < DOCX_PAGE_SIZES.length; i++) {
      if (DOCX_PAGE_SIZES[i].w === sw && DOCX_PAGE_SIZES[i].h === sh) { return DOCX_PAGE_SIZES[i].name; }
    }
    return (sw && sh) ? "custom" : null;
  }
  /* 路径规范化 / rels 路径换算(包内一律用 / 分隔、不含前导 /) */
  function normalizePart(p) {
    var segs = String(p).split("/"), out = [], i;
    for (i = 0; i < segs.length; i++) {
      if (segs[i] === "" || segs[i] === ".") { continue; }
      if (segs[i] === "..") { out.pop(); continue; }
      out.push(segs[i]);
    }
    return out.join("/");
  }
  function resolvePartName(baseDir, target) {
    var t = String(target == null ? "" : target);
    if (t.charAt(0) === "/") { return normalizePart(t.slice(1)); }
    return normalizePart((baseDir ? baseDir + "/" : "") + t);
  }
  function relsPathOf(part) {
    var s = String(part), i = s.lastIndexOf("/");
    var dir = i < 0 ? "" : s.slice(0, i);
    var base = i < 0 ? s : s.slice(i + 1);
    return (dir ? dir + "/" : "") + "_rels/" + base + ".rels";
  }
  function relsDirOf(relsPart) {
    var s = String(relsPart), i = s.lastIndexOf("/_rels/");
    return i < 0 ? "" : s.slice(0, i);
  }
  function extOf(name) {
    var s = String(name), i = s.lastIndexOf(".");
    return i < 0 ? "" : s.slice(i + 1).toLowerCase();
  }
  function isDirEntry(name) { var s = String(name); return s.charAt(s.length - 1) === "/"; }
  function mainPartOf(fileType) { return fileType === "pptx" ? PART.presentation : PART.document; }
  function normFileType(v) {
    var s = String(v == null ? "" : v).toLowerCase();
    return (s === "docx" || s === "pptx") ? s : "";
  }

  /* ---------------- 包读写助手(纯数据变换,不碰工作区) ---------------- */
  function asBytes(u8) {
    if (u8 instanceof Uint8Array) { return u8; }
    if (u8 && typeof u8.length === "number") { return new Uint8Array(u8); }
    return null;
  }
  function loadDraft(bytes) {
    var u8 = asBytes(bytes);
    if (!u8 || !u8.length) { return fail("EINVAL", "编辑类 operation 需要输入文件内容(bytes 为空)"); }
    var un = zipUnzip(u8);
    if (!un.ok) { return un; }
    return { ok: true, entries: un.name2bytes, order: un.order, meta: un.meta, size: u8.length };
  }
  function partText(entries, name) { return has(entries, name) ? bytesToText(entries[name]) : null; }
  /* 通用:取部件 → 解析 → 带上原文件的声明(批次 B 的 docxPartOf 就是它的 docx 别名) */
  function partDocOf(entries, partName) {
    var raw = partText(entries, partName);
    if (raw == null) { return fail("EOFFICE", "缺少部件 " + partName); }
    var par = xmlParse(raw);
    if (!par.ok) { return fail(par.code, partName + " 解析失败:" + par.error); }
    return { ok: true, doc: par.doc, decl: xmlDeclOf(raw), raw: raw };
  }
  function docxPartOf(entries, partName) { return partDocOf(entries, partName); }
  /* 是不是**我们生成的** pptx(判据 = docProps/app.xml 的 Application 文本)——
     add_slide 靠它决定"用 layout 参数指定的版式"还是"沿用最后一页的版式" */
  function pptxIsOwnDeck(entries) {
    var d = partDocOf(entries, PART.app);
    if (!d.ok) { return false; }
    var list = d.doc.getElementsByTagNameNS(NS.ep, "Application");
    if (!list || !list.length) { return false; }
    return String(xmlText(list[0])) === "AzusaAI WebUI";
  }
  /* 原包 + 本次改写 = 新包;**写盘前**过 validatePackage 的 ①–④(§12.8/A16)。
     requireAll=true(create)才要求固定件齐 —— 编辑类的既有文件是别人的,不能拿
     我们的固定清单去判它(§2.4 的固定 7 件是 create 的产出承诺)。
     批次 C 追加两个约定:
       · `writes[k] === null` = **删除该部件**(pptx delete 去条目的唯一通道)
       · addedParts = 「本次新增件」清单(pptx 的每张幻灯 2 件;create 时与固定件一起判)
     批次 F-2 追加(P2-1):
       · addedGraphics = 「本次新增的 a:graphicData」清单,条目 = { part, uri, rid, hostId }
         (rid = 宿主里指向新增关系的 r:id / r:embed;hostId = pptx 宿主形状的 p:cNvPr/@id,
          给**没有关系引用**的宿主用 —— 表格 a:tbl 就是这一类,批 H 起它不再走"只按 uri
          定位"那条偏松的路)。⑦ 的 uri 白名单**只**核这几个节点 —— 不再按"改写过的宿主
         部件"收窄,因为 SmartArt / OLE 恰恰可能就在我们改写的那个部件里(改前会误判 EOFFICE) */
  function finishDraft(pkg, writes, fileType, requireAll, opts, addedParts, addedGraphics) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    progress(opts, "validate");
    var entries = {}, k;
    for (k in pkg.entries) { if (has(pkg.entries, k)) { entries[k] = pkg.entries[k]; } }
    /* 本次改写统一先转成字节:改写内容在 builder 里是字符串,而 validateEntries 会按
       "部件名 → 字节" 读全部条目(bytesToText),zipBuild 也一律吃字节 */
    var dropped = {};
    for (k in writes) {
      if (!has(writes, k)) { continue; }
      if (writes[k] === null) { dropped[k] = true; if (has(entries, k)) { delete entries[k]; } continue; }
      entries[k] = (typeof writes[k] === "string") ? textToBytes(writes[k]) : writes[k];
    }
    var order = pkg.order.slice();
    for (k in writes) {
      if (!has(writes, k) || dropped[k]) { continue; }
      if (order.indexOf(k) < 0) { order.push(k); }
    }
    if (Object.keys(dropped).length) {
      var keep = [], i;
      for (i = 0; i < order.length; i++) { if (!dropped[order[i]]) { keep.push(order[i]); } }
      order = keep;
    }
    var v = validateEntries(entries, fileType,
      { requireAll: !!requireAll, addedParts: addedParts || [], addedGraphics: addedGraphics || [] });
    if (!v.ok) { return v; }
    progress(opts, "pack");
    var z = zipBuild(entries, order);
    if (!z.ok) { return z; }
    return { ok: true, bytes: z.bytes, checked: v.checked, pending: v.pending };
  }
  function okResult(bytes, fileType, counts, warnings, steps) {
    return {
      ok: true, bytes: bytes, mime: MIME[fileType], size: bytes.length,
      counts: counts || {}, warnings: warnings || [], steps: steps || []
    };
  }

  /* ---------------- S12:docProps/core.xml 的字段读写 ---------------- */
  /* cp:coreProperties 的子元素顺序(顺序错 Word 会提示"修复");只列我们写的五个字段
     + 模板里已有的时间/修订号,其余元素按"排在已知项之后"处理 */
  var CORE_FIELDS = [
    { key: "title", ns: NS.dc, q: "dc:title", local: "title" },
    { key: "subject", ns: NS.dc, q: "dc:subject", local: "subject" },
    { key: "creator", ns: NS.dc, q: "dc:creator", local: "creator" },
    { key: "keywords", ns: NS.cp, q: "cp:keywords", local: "keywords" },
    { key: "description", ns: NS.dc, q: "dc:description", local: "description" },
    { key: "lastModifiedBy", ns: NS.cp, q: "cp:lastModifiedBy", local: "lastModifiedBy" },
    { key: "revision", ns: NS.cp, q: "cp:revision", local: "revision" },
    { key: "created", ns: NS.dcterms, q: "dcterms:created", local: "created" },
    { key: "modified", ns: NS.dcterms, q: "dcterms:modified", local: "modified" }
  ];
  /* set_properties 能写的字段(= inputSchema 的 properties) */
  var CORE_WRITABLE = ["title", "creator", "subject", "description", "keywords"];
  function coreFieldDef(key) {
    for (var i = 0; i < CORE_FIELDS.length; i++) { if (CORE_FIELDS[i].key === key) { return CORE_FIELDS[i]; } }
    return null;
  }
  function coreOrderOf(ns, local) {
    for (var i = 0; i < CORE_FIELDS.length; i++) {
      if (CORE_FIELDS[i].ns === ns && CORE_FIELDS[i].local === local) { return i; }
    }
    return CORE_FIELDS.length;
  }
  function coreFind(root, ns, local) {
    var kids = root.childNodes;
    for (var i = 0; i < kids.length; i++) {
      var n = kids[i];
      if (n.nodeType === 1 && n.namespaceURI === ns && n.localName === local) { return n; }
    }
    return null;
  }
  function coreSet(doc, root, def, value) {
    var el = coreFind(root, def.ns, def.local), i;
    if (!el) {
      el = elNs(doc, def.ns, def.q);
      if (def.ns === NS.dcterms) { el.setAttributeNS(NS.xsi, "xsi:type", "dcterms:W3CDTF"); }
      var want = coreOrderOf(def.ns, def.local), anchor = null, kids = root.childNodes;
      for (i = 0; i < kids.length; i++) {
        var n = kids[i];
        if (n.nodeType !== 1) { continue; }
        if (coreOrderOf(n.namespaceURI, n.localName) > want) { anchor = n; break; }
      }
      if (anchor) { root.insertBefore(el, anchor); } else { root.appendChild(el); }
    }
    return setElemText(doc, el, value == null ? "" : value);
  }
  function coreApply(doc, props, withTimestamp) {
    var root = doc.documentElement;
    if (!root || root.namespaceURI !== NS.cp || root.localName !== "coreProperties") {
      return fail("EOFFICE", PART.core + " 的根元素不是 cp:coreProperties");
    }
    var p = props || {}, i, wrote = 0;
    for (i = 0; i < CORE_WRITABLE.length; i++) {
      if (!has(p, CORE_WRITABLE[i])) { continue; }
      coreSet(doc, root, coreFieldDef(CORE_WRITABLE[i]), p[CORE_WRITABLE[i]]);
      wrote++;
    }
    if (withTimestamp) {
      var now = isoNow();
      coreSet(doc, root, coreFieldDef("created"), now);
      coreSet(doc, root, coreFieldDef("modified"), now);
    }
    return { ok: true, value: { wrote: wrote } };
  }
  function setAppParagraphs(doc, n) {
    var root = doc.documentElement;
    if (!root) { return fail("EOFFICE", PART.app + " 根元素缺失"); }
    var list = root.getElementsByTagNameNS(NS.ep, "Paragraphs");
    var el = (list && list.length) ? list[0] : null;
    if (!el) {
      el = elNs(doc, NS.ep, "Paragraphs");
      root.appendChild(el);
    }
    setElemText(doc, el, String(n));
    return { ok: true, value: { paragraphs: n } };
  }
  /* 取模板件 → 解析 → 改写(可选)→ 用**模板自己那份声明**序列化
     (批次 C:pptx 走同一个 tplPartOf,只是换模板表 —— 避免另起一套字符串拼接) */
  function tplPartOf(tbl, label, key, mutate) {
    var t = tbl[key];
    if (typeof t !== "string") { return fail("EINTERNAL", "缺少 " + label + " 模板部件 " + key); }
    var par = xmlParse(t);
    if (!par.ok) { return par; }
    var value = null;
    if (mutate) {
      var m = mutate(par.doc);
      if (m && m.ok === false) { return m; }
      value = m;
    }
    var s = xmlSerialize(par.doc, xmlDeclOf(t));
    if (!s.ok) { return s; }
    return { ok: true, text: s.text, value: value };
  }
  function tplDocOf(key, mutate) { return tplPartOf(TPL_DOCX, "docx", key, mutate); }
  function tplPptxOf(key, mutate) { return tplPartOf(TPL_PPTX, "pptx", key, mutate); }
  /* CT 里确保有某个 Override(缺则追加);返回的 text 为 undefined 表示无需改写 */
  function ctEnsureOverride(entries, partName, contentType, tplText) {
    var raw = partText(entries, PART.contentTypes);
    if (raw == null) { return fail("EOFFICE", "缺少部件 " + PART.contentTypes); }
    var par = xmlParse(raw);
    if (!par.ok) { return fail(par.code, PART.contentTypes + " 解析失败:" + par.error); }
    var doc = par.doc, root = doc.documentElement;
    var ovs = root.getElementsByTagNameNS(NS.ct, "Override"), i;
    for (i = 0; i < ovs.length; i++) {
      if (String(ovs[i].getAttribute("PartName") || "") === "/" + partName) { return { ok: true }; }
    }
    var ov = elNs(doc, NS.ct, "Override");
    ov.setAttribute("PartName", "/" + partName);
    ov.setAttribute("ContentType", contentType);
    root.appendChild(ov);
    var s = xmlSerialize(doc, xmlDeclOf(raw));
    if (!s.ok) { return s; }
    return { ok: true, text: s.text, changed: true };
  }
  /* CT 里去掉某个 Override(pptx delete 去第 4 处引用之一);返回 changed=false 表示本来就没有 */
  function ctRemoveOverride(entries, partName) {
    var raw = partText(entries, PART.contentTypes);
    if (raw == null) { return fail("EOFFICE", "缺少部件 " + PART.contentTypes); }
    var par = xmlParse(raw);
    if (!par.ok) { return fail(par.code, PART.contentTypes + " 解析失败:" + par.error); }
    var doc = par.doc, root = doc.documentElement;
    var ovs = root.getElementsByTagNameNS(NS.ct, "Override"), hit = null, i;
    for (i = 0; i < ovs.length; i++) {
      if (String(ovs[i].getAttribute("PartName") || "") === "/" + partName) { hit = ovs[i]; break; }
    }
    if (!hit) { return { ok: true, changed: false }; }
    hit.parentNode.removeChild(hit);
    var s = xmlSerialize(doc, xmlDeclOf(raw));
    if (!s.ok) { return s; }
    return { ok: true, changed: true, text: s.text };
  }
  /* 某个 rels 里确保有某条关系(缺则追加 + 用未占用的 rId);包级 _rels/.rels 缺失时
     用模板补一份(fallback 缺省 = docx 的 rootRels;pptx 传 TPL_PPTX.rootRels) */
  function relsEnsure(entries, relsPart, type, target, fallback) {
    var fb = fallback || TPL_DOCX.rootRels;
    var raw = partText(entries, relsPart);
    if (raw == null) {
      if (relsPart !== PART.rootRels) { return fail("EOFFICE", "缺少部件 " + relsPart); }
      raw = fb;
    }
    var par = xmlParse(raw);
    if (!par.ok) { return fail(par.code, relsPart + " 解析失败:" + par.error); }
    var doc = par.doc, root = doc.documentElement;
    var rels = root.getElementsByTagNameNS(NS.pr, "Relationship");
    var max = 0, i, id, m;
    for (i = 0; i < rels.length; i++) {
      if (String(rels[i].getAttribute("Type") || "") === type) { return { ok: true, text: raw === fb ? raw : undefined }; }
      id = String(rels[i].getAttribute("Id") || "");
      m = /^rId([0-9]+)$/.exec(id);
      if (m && parseInt(m[1], 10) > max) { max = parseInt(m[1], 10); }
    }
    var rel = elNs(doc, NS.pr, "Relationship");
    rel.setAttribute("Id", "rId" + (max + 1));
    rel.setAttribute("Type", type);
    rel.setAttribute("Target", target);
    root.appendChild(rel);
    var s = xmlSerialize(doc, xmlDeclOf(raw));
    if (!s.ok) { return s; }
    return { ok: true, text: s.text };
  }
  /* cp:coreProperties 缺失时补件(CT Override + 包级 _rels/.rels 关系)——
     docx / pptx 的 set_properties 共用;模板 core 与新关系都按 fileType 取 */
  function corePartEnsure(entries, fileType) {
    var isP = (fileType === "pptx");
    var ct = ctEnsureOverride(entries, PART.core, isP ? CT_PPTX.core : CT_DOCX.core);
    if (!ct.ok) { return ct; }
    var rl = relsEnsure(entries, PART.rootRels, REL_CORE, PART.core, isP ? TPL_PPTX.rootRels : TPL_DOCX.rootRels);
    if (!rl.ok) { return rl; }
    return { ok: true, ct: ct.text, rels: rl.text };
  }

  /* ---------------- S8 / S10:正文块构造 ---------------- */
  /* 把块列表挂进 w:body(插在 body 级 w:sectPr **之前** —— 节属性必须留在末尾) */
  function docxBuildBody(doc, blocks) {
    var body = docxBody(doc);
    if (!body) { return fail("EOFFICE", PART.document + " 缺少 w:body"); }
    var sectPr = docxBodySectPr(body), made = 0, i, p;
    for (i = 0; i < blocks.length; i++) {
      p = wParagraph(doc, blocks[i]);
      if (sectPr) { body.insertBefore(p, sectPr); } else { body.appendChild(p); }
      made++;
    }
    return { ok: true, value: { paragraphs: made } };
  }

  /* ---------------- S9:outline(docx) ---------------- */
  /* 末节的页眉 / 页脚文本(无 → null):sectPr 的 w:headerReference →
     word/_rels/document.xml.rels → w:hdr / w:ftr 的 w:t 拼接。
     单值字段**不截断**(方案 §2.8 的 "≤80 字符" 是给段落预览的;T14 要求文本全等)。
     批次 G:同一个 sectPr 可以有多条同类引用(默认 / 首页 / 偶数页三种),**取
     w:type="default"** 那一条(缺 w:type 等同 default);都没有才退回第一条 ——
     否则 outline 报的文本会是"首页"或"偶数页"那一套(T14 要求 == 输入) */
  function docxHeaderFooter(entries, doc, which) {
    var body = docxBody(doc);
    var sectPr = body ? docxBodySectPr(body) : null;
    if (!sectPr) { return null; }
    var refs = sectPr.getElementsByTagNameNS(NS.w, which + "Reference");
    if (!refs || !refs.length) { return null; }
    var pick = null, i;
    for (i = 0; i < refs.length; i++) {
      var ty = String(refs[i].getAttributeNS(NS.w, "type") || "default");
      if (ty === "default") { pick = refs[i]; break; }
      if (!pick) { pick = refs[i]; }
    }
    var raw = partText(entries, PART.documentRels);
    if (raw == null) { return null; }
    var rp = xmlParse(raw);
    if (!rp.ok) { return null; }
    var rels = rp.doc.getElementsByTagNameNS(NS.pr, "Relationship"), j;
    var text = null, target, part, hraw, hp;
    var rid = pick.getAttributeNS(NS.r, "id");
    if (!rid) { return null; }
    for (j = 0; j < rels.length; j++) {
      if (String(rels[j].getAttribute("Id") || "") !== rid) { continue; }
      target = String(rels[j].getAttribute("Target") || "");
      if (!target) { continue; }
      part = resolvePartName("word", target);
      hraw = partText(entries, part);
      if (hraw == null) { continue; }
      hp = xmlParse(hraw);
      if (!hp.ok) { continue; }
      if (text == null) { text = wTextsOf(hp.doc.documentElement); }
    }
    return text == null ? null : { text: text };
  }
  function docxOutlineOf(entries, doc, spec) {
    var items = docxItems(doc), out = [], paras = 0, tables = 0, i, n, kind, style, text;
    for (i = 0; i < items.length; i++) {
      n = items[i];
      kind = "other";
      style = "";
      text = "";
      if (n.localName === "p") { kind = "paragraph"; style = docxPStyle(n); text = wTextsOf(n); paras++; }
      else if (n.localName === "tbl") { kind = "table"; text = docxTablePreview(n); tables++; }
      out.push({ i: i, kind: kind, style: style, text: brief(text, 80) });
    }
    var limit = 80;
    if (spec && spec.head_limit != null) {
      limit = parseInt(spec.head_limit, 10);
      if (isNaN(limit) || limit < 1) { limit = 80; }
    }
    if (limit > 200) { limit = 200; }
    return {
      ok: true,
      outline: {
        items: out.slice(0, limit), total: out.length, truncated: out.length > limit, head_limit: limit,
        header: docxHeaderFooter(entries, doc, "header"),
        footer: docxHeaderFooter(entries, doc, "footer"),
        sections: { count: (doc.getElementsByTagNameNS(NS.w, "sectPr") || []).length },
        page: docxPageOf(doc)
      },
      counts: { paragraphs: paras, tables: tables }
    };
  }

  /* ---------------- S11:replace_text(docx,两阶段) ---------------- */
  function docxReplaceIn(doc, spec) {
    var oldS = String(spec.old_string);
    var newS = (spec.new_string == null) ? "" : String(spec.new_string);
    var all = (spec.replace_all !== false);
    var body = docxBody(doc);
    if (!body) { return fail("EOFFICE", PART.document + " 缺少 w:body"); }
    var ts = body.getElementsByTagNameNS(NS.w, "t"), i, j;
    var hits = 0, total = 0, cur, occ;
    var warnings = [], replaced = 0;
    /* ①(第一阶段)单 run 精确命中 —— 直接改 w:t 文本,格式零损失 */
    for (i = 0; i < ts.length; i++) {
      occ = occCount(xmlText(ts[i]), oldS);
      if (occ > 0) { hits++; total += occ; }
    }
    if (hits > 0) {
      if (!all && total > 1) {
        return fail("EINVAL", "old_string 命中 " + total + " 处,replace_all=false 要求唯一命中");
      }
      for (i = 0; i < ts.length; i++) {
        cur = xmlText(ts[i]);
        if (cur.indexOf(oldS) < 0) { continue; }
        if (all) {
          replaced += occCount(cur, oldS);
          setElemText(doc, ts[i], cur.split(oldS).join(newS));
        } else if (replaced === 0) {
          var at = cur.indexOf(oldS);
          setElemText(doc, ts[i], cur.slice(0, at) + newS + cur.slice(at + oldS.length));
          replaced++;
        }
      }
      return { ok: true, value: { replaced: replaced, warnings: warnings, crossRun: 0 } };
    }
    /* ②(第二阶段)0 命中且**某段的 run 拼接串**含 old_string ⇒ 段落级重写:
       写回首个 w:t、清空其余 w:t(保留 run 节点与其 rPr),并逐个推 warnings */
    var paras = body.getElementsByTagNameNS(NS.w, "p"), plan = [], pt, joined;
    for (j = 0; j < paras.length; j++) {
      pt = paras[j].getElementsByTagNameNS(NS.w, "t");
      joined = "";
      for (i = 0; i < pt.length; i++) { joined += xmlText(pt[i]); }
      if (joined.indexOf(oldS) < 0) { continue; }
      plan.push({ pi: j, ts: pt, joined: joined, occ: occCount(joined, oldS) });
    }
    var total2 = 0;
    for (j = 0; j < plan.length; j++) { total2 += plan[j].occ; }
    if (total2 > 0 && !all && total2 > 1) {
      return fail("EINVAL", "old_string 命中 " + total2 + " 处(跨 run),replace_all=false 要求唯一命中");
    }
    var left = all ? -1 : 1, it, nextText, at2;
    for (j = 0; j < plan.length; j++) {
      it = plan[j];
      nextText = it.joined;
      if (all) {
        nextText = nextText.split(oldS).join(newS);
        replaced += it.occ;
      } else if (left > 0) {
        at2 = nextText.indexOf(oldS);
        nextText = nextText.slice(0, at2) + newS + nextText.slice(at2 + oldS.length);
        replaced++;
        left--;
      }
      if (it.ts.length) { setElemText(doc, it.ts[0], nextText); }
      for (i = 1; i < it.ts.length; i++) { setElemText(doc, it.ts[i], ""); }
      warnings.push("第 " + it.pi + " 段命中跨 run,已按整段重写,该段 run 级格式可能变化");
    }
    if (replaced === 0) { warnings.push("未找到 old_string,文档未改动"); }
    return { ok: true, value: { replaced: replaced, warnings: warnings, crossRun: plan.length } };
  }

  /* ---------------- S12:delete(docx) ---------------- */
  function docxDeleteIn(doc, spec) {
    var idx = spec.paragraph_index;
    if (typeof idx !== "number" || isNaN(idx) || idx < 0 || Math.floor(idx) !== idx) {
      return fail("EINVAL", "paragraph_index 必须是不小于 0 的整数");
    }
    var items = docxItems(doc);
    if (idx >= items.length) {
      return fail("EINVAL", "paragraph_index " + idx + " 越界(正文共 " + items.length + " 项)");
    }
    var n = items[idx];
    if (n.localName !== "p") {
      return fail("EINVAL", "paragraph_index " + idx + " 指向的不是段落(是 w:" + n.localName + "),delete 只支持删段落");
    }
    n.parentNode.removeChild(n);
    return { ok: true, value: { deleted: 1 } };
  }

  /* ---------------- 各 operation 的落地 ---------------- */
  function docxCreate(spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    progress(opts, "parse");
    var blocks = parseContentLite(spec.content);
    if (!blocks.length) { return fail("EINVAL", "content 为空:至少给一段正文"); }
    progress(opts, "build");
    var parts = {}, i;
    var docPart = tplDocOf("document", function (doc) { return docxBuildBody(doc, blocks); });
    if (!docPart.ok) { return docPart; }
    var corePart = tplDocOf("core", function (doc) { return coreApply(doc, spec.props, true); });
    if (!corePart.ok) { return corePart; }
    var nPara = (docPart.value && docPart.value.paragraphs) || blocks.length;
    var appPart = tplDocOf("app", function (doc) { return setAppParagraphs(doc, nPara); });
    if (!appPart.ok) { return appPart; }
    parts[PART.document] = docPart.text;
    parts[PART.core] = corePart.text;
    parts[PART.app] = appPart.text;
    /* 其余固定件原样落盘(不解析不序列化 ⇒ 字节稳定);按需件(document.xml.rels)
       这一批不产出:纯文本 create 没有任何跨部件引用 */
    parts[PART.contentTypes] = TPL_DOCX.contentTypes;
    parts[PART.rootRels] = TPL_DOCX.rootRels;
    parts[PART.styles] = TPL_DOCX.styles;
    parts[PART.numbering] = TPL_DOCX.numbering;
    var pkg = { entries: {}, order: [], meta: { zip64: false, comment: "" } };
    var fin = finishDraft(pkg, parts, "docx", true, opts);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "docx", { paragraphs: nPara },
      [], ["生成 docx 固定件 7 件", "正文 " + nPara + " 段", "包结构自检 " + fin.checked.length + " 条通过"]);
  }

  function docxOutline(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var d = docxPartOf(pkg.entries, PART.document);
    if (!d.ok) { return d; }
    var r = docxOutlineOf(pkg.entries, d.doc, spec);
    progress(opts, "done");
    /* outline **不产出新字节**:结果里不给 bytes(免得调用方误把它写回工作区) */
    return {
      ok: true, readOnly: true, mime: MIME.docx, size: 0,
      outline: r.outline, counts: r.counts, warnings: [],
      steps: ["outline:正文 " + r.outline.total + " 项(返回 " + r.outline.items.length + " 项)"]
    };
  }

  function docxAppend(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var hasPara = spec.paragraphs != null, hasCont = spec.content_append != null;
    if (hasPara && hasCont) { return fail("EINVAL", "paragraphs 与 content_append 只能给一个"); }
    var blocks = [], i;
    if (hasCont) {
      blocks = parseContentLite(spec.content_append);
    } else if (hasPara) {
      if (!(spec.paragraphs instanceof Array)) { return fail("EINVAL", "paragraphs 必须是字符串数组"); }
      for (i = 0; i < spec.paragraphs.length; i++) {
        blocks.push({ kind: "p", text: String(spec.paragraphs[i] == null ? "" : spec.paragraphs[i]) });
      }
    }
    if (!blocks.length) { return fail("EINVAL", "append 需要 paragraphs 或 content_append(且不能为空)"); }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var d = docxPartOf(pkg.entries, PART.document);
    if (!d.ok) { return d; }
    progress(opts, "build");
    var b = docxBuildBody(d.doc, blocks);
    if (!b.ok) { return b; }
    var s = xmlSerialize(d.doc, d.decl);
    if (!s.ok) { return s; }
    var writes = {};
    writes[PART.document] = s.text;
    var fin = finishDraft(pkg, writes, "docx", false, opts);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "docx", { paragraphs: blocks.length, appended: blocks.length },
      [], ["在正文末尾(w:sectPr 之前)追加 " + blocks.length + " 段"]);
  }

  function docxReplaceText(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    if (typeof spec.old_string !== "string" || !spec.old_string.length) {
      return fail("EINVAL", "old_string 必须是非空字符串");
    }
    progress(opts, "parse");
    var u8 = asBytes(bytes);
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var d = docxPartOf(pkg.entries, PART.document);
    if (!d.ok) { return d; }
    progress(opts, "build");
    var m = docxReplaceIn(d.doc, spec);
    if (!m.ok) { return m; }
    var steps = m.value.crossRun
      ? ["单 run 精确命中 0 处,跨 run 段落级重写 " + m.value.crossRun + " 段"]
      : ["单 run 精确命中替换 " + m.value.replaced + " 处"];
    /* 0 命中:不重打包 —— 原样回传输入字节(调用方写回后文件 md5 不变) */
    if (m.value.replaced === 0) {
      return {
        ok: true, bytes: u8, mime: MIME.docx, size: u8.length, unchanged: true,
        counts: { replaced: 0 }, warnings: m.value.warnings, steps: steps
      };
    }
    var s = xmlSerialize(d.doc, d.decl);
    if (!s.ok) { return s; }
    var writes = {};
    writes[PART.document] = s.text;
    var fin = finishDraft(pkg, writes, "docx", false, opts);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "docx", { replaced: m.value.replaced }, m.value.warnings, steps);
  }

  function docxDelete(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var d = docxPartOf(pkg.entries, PART.document);
    if (!d.ok) { return d; }
    progress(opts, "build");
    var m = docxDeleteIn(d.doc, spec);
    if (!m.ok) { return m; }
    var s = xmlSerialize(d.doc, d.decl);
    if (!s.ok) { return s; }
    var writes = {};
    writes[PART.document] = s.text;
    var fin = finishDraft(pkg, writes, "docx", false, opts);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "docx", { deleted: 1 },
      [], ["删除内容序号 " + spec.paragraph_index + " 的段落"]);
  }

  function docxSetProperties(bytes, spec, opts) { return propsSetCore(bytes, "docx", spec, opts); }
  /* docx / pptx 的 set_properties 只有"模板件 + CT 串"两处不同(方案 §3 S12 / S18;
     pptx 侧 app.xml 的 <Slides> 一律**不改** —— 见 S18),故合成一个入口 */
  function propsSetCore(bytes, fileType, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var props = spec.props || {}, i, wrote = 0;
    for (i = 0; i < CORE_WRITABLE.length; i++) { if (has(props, CORE_WRITABLE[i])) { wrote++; } }
    if (!wrote) {
      return fail("EINVAL", "properties 里没有任何可写字段(支持 "
        + CORE_WRITABLE.join(" / ") + ")");
    }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    progress(opts, "build");
    /* docProps/core.xml 是格式无关部件(cp:coreProperties 两边同型)⇒ 模板只有一份 */
    var tplCore = TPL_DOCX.core;
    var raw = partText(pkg.entries, PART.core);
    var isNew = (raw == null);
    if (isNew) { raw = tplCore; }
    var par = xmlParse(raw);
    if (!par.ok) { return fail(par.code, PART.core + " 解析失败:" + par.error); }
    var m = coreApply(par.doc, props, false);
    if (!m.ok) { return m; }
    var s = xmlSerialize(par.doc, xmlDeclOf(raw));
    if (!s.ok) { return s; }
    var writes = {}, steps = [];
    writes[PART.core] = s.text;
    if (isNew) {
      var en = corePartEnsure(pkg.entries, fileType);
      if (!en.ok) { return en; }
      if (en.ct != null) { writes[PART.contentTypes] = en.ct; }
      if (en.rels != null && en.rels !== partText(pkg.entries, PART.rootRels)) { writes[PART.rootRels] = en.rels; }
      steps.push("新建 " + PART.core + " + 补 Content_Types Override + 包级 _rels/.rels 关系");
    }
    steps.push("写入 " + wrote + " 个属性字段");
    var fin = finishDraft(pkg, writes, fileType, false, opts);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, fileType, {}, [], steps);
  }

  /* ============================================================
     批次 C:pptx 生成与编辑(方案 §3 的 S13–S18)
     分层照批次 B(tplDocOf 式「模板 → DOM 改写 → 用模板自己的声明序列化」):
       模板装配(S13)→ 幻灯构造(S14)→ 索引(S15)→ 整段改写(S16)→
       追加 / 删除 / 两阶段替换(S17)→ 写属性(S18)
     DOM 构造三陷阱(同 §3 S8):节点一律用**该部件自己的 doc**.createElementNS、
     只构造 DOM 不拼 XML 字符串、文本一律 createTextNode(需要保空格时 xml:space)。
     属性命名空间口径(与真值一致,别照抄 docx 的 wAttr):
       · p:/a: 元素上的 `type` / `idx` / `sz` / `id` / `name` / `x` / `cx` … = **无命名空间**
         (用 setAttribute → nAttr),只有 `r:id` / `r:embed` 带 r: 前缀。
     ============================================================ */

  /* ---------------- pptx DOM 助手 ---------------- */
  function elP(doc, name) { return doc.createElementNS(NS.p, "p:" + name); }
  function elA(doc, name) { return doc.createElementNS(NS.a, "a:" + name); }
  /* 无命名空间属性(p:/a: 元素上的绝大多数属性都是这种形态) */
  function nAttr(el, name, val) { el.setAttribute(name, String(val)); return el; }
  function rAttr(el, name, val) { return attrNS(el, NS.r, "r:" + name, val); }
  function aText(doc, s) { return setElemText(doc, elA(doc, "t"), s); }
  /* 一个 a:p(空文本 ⇒ 空段,与真值 `<a:p/>` 一致) */
  function pptxPara(doc, s) {
    var p = elA(doc, "p");
    var t = String(s == null ? "" : s);
    if (!t.length) { return p; }
    var r = elA(doc, "r");
    var rPr = elA(doc, "rPr");
    nAttr(rPr, "lang", "zh-CN");
    nAttr(rPr, "altLang", "en-US");
    r.appendChild(rPr);
    r.appendChild(aText(doc, t));
    p.appendChild(r);
    return p;
  }
  function pptxTxBody(doc, lines) {
    var tb = elP(doc, "txBody");
    tb.appendChild(elA(doc, "bodyPr"));
    tb.appendChild(elA(doc, "lstStyle"));
    var arr = (lines && lines.length) ? lines : [""];
    for (var i = 0; i < arr.length; i++) { tb.appendChild(pptxPara(doc, arr[i])); }
    return tb;
  }
  function pptxTxBodyOf(el) {
    var kids = el.childNodes, i;
    for (i = 0; i < kids.length; i++) {
      if (kids[i].nodeType === 1 && kids[i].namespaceURI === NS.p && kids[i].localName === "txBody") {
        return kids[i];
      }
    }
    return null;
  }
  /* p:ph 的无命名空间属性(type / sz / idx)复制 —— 跨部件只读属性值,不搬节点 */
  function pptxPhClone(doc, ph) {
    var el = elP(doc, "ph"), attrs = ph.attributes, i;
    for (i = 0; i < attrs.length; i++) {
      if (attrs[i].namespaceURI) { continue; }
      el.setAttribute(attrs[i].name, attrs[i].value);
    }
    return el;
  }

  /* ---------------- pptx 结构读取 ---------------- */
  function pptxSpTree(doc) {
    var st = doc.getElementsByTagNameNS(NS.p, "spTree");
    return (st && st.length) ? st[0] : null;
  }
  /* 形状 = p:spTree 的**直接子元素**里的形状类元素(下表外的子元素不算)——
     与 outline 的 shapes[].i / set_text 的 shape_index 同一个序号口径 */
  var PPTX_SHAPE_TAGS = { sp: 1, pic: 1, graphicFrame: 1, grpSp: 1, cxnSp: 1, contentPart: 1 };
  function pptxShapesOf(doc) {
    var st = pptxSpTree(doc), out = [], kids, i;
    if (!st) { return out; }
    kids = st.childNodes;
    for (i = 0; i < kids.length; i++) {
      var n = kids[i];
      if (n.nodeType === 1 && n.namespaceURI === NS.p && PPTX_SHAPE_TAGS[n.localName]) { out.push(n); }
    }
    return out;
  }
  /* 形状自己的 p:ph(只认 nvSpPr / nvPicPr / nvGraphicFramePr / nvCxnSpPr 下的那一个,
     不递归到子形状) */
  function pptxPhOf(el) {
    var kids = el.childNodes, i, j, k, nv, nk, nvp, pk, ph;
    for (i = 0; i < kids.length; i++) {
      nv = kids[i];
      if (nv.nodeType !== 1 || nv.namespaceURI !== NS.p) { continue; }
      if (nv.localName !== "nvSpPr" && nv.localName !== "nvPicPr"
        && nv.localName !== "nvGraphicFramePr" && nv.localName !== "nvCxnSpPr") { continue; }
      nk = nv.childNodes;
      for (j = 0; j < nk.length; j++) {
        nvp = nk[j];
        if (nvp.nodeType !== 1 || nvp.namespaceURI !== NS.p || nvp.localName !== "nvPr") { continue; }
        pk = nvp.childNodes;
        for (k = 0; k < pk.length; k++) {
          ph = pk[k];
          if (ph.nodeType === 1 && ph.namespaceURI === NS.p && ph.localName === "ph") { return ph; }
        }
      }
    }
    return null;
  }
  function pptxPhType(ph) { return String(ph.getAttribute("type") == null ? "" : ph.getAttribute("type")); }
  function pptxPhIdx(ph) {
    var v = ph.getAttribute("idx");
    return (v && /^[0-9]+$/.test(String(v))) ? parseInt(v, 10) : 0;
  }
  /* 内容型占位符(批 C 只搬这些;dt / ftr / sldNum / hdr 不进幻灯 —— 与 PowerPoint
     新建幻灯的行为一致:那三个占位符由 set_header_footer(批次 G)在需要时**逐张**
     落到幻灯上,形态见 §13.3 F2) */
  var PPTX_TEXT_PH = { "": 1, title: 1, ctrTitle: 1, body: 1, subTitle: 1, obj: 1 };
  /* kind 判据(方案 §3 S15):有 p:ph 时按 ph/@type;否则按元素名 / graphicData 的 uri */
  function pptxKindOf(el) {
    var ph = pptxPhOf(el), t;
    if (ph) {
      t = pptxPhType(ph);
      if (t === "title" || t === "ctrTitle") { return "title"; }
      if (PPTX_TEXT_PH[t]) { return "body"; }
      return "other";
    }
    if (el.localName === "pic") { return "pic"; }
    if (el.localName === "graphicFrame") {
      var gd = el.getElementsByTagNameNS(NS.a, "graphicData");
      var uri = (gd && gd.length) ? String(gd[0].getAttribute("uri") || "") : "";
      if (uri.indexOf("/chart") >= 0) { return "chart"; }
      if (uri.indexOf("/table") >= 0) { return "table"; }
      return "other";
    }
    if (el.localName === "sp") { return "text"; }
    return "other";
  }
  /* 形状文本:优先 txBody 的 a:p 逐段拼接(段间 \n);无 txBody 的元素(表格 / 图形)
     退回"该元素下全部 a:t 按文档序拼接" */
  function pptxTextOf(el) {
    var tb = pptxTxBodyOf(el), i, j, ts, s;
    if (!tb) {
      ts = el.getElementsByTagNameNS(NS.a, "t");
      s = "";
      for (i = 0; i < ts.length; i++) { s += xmlText(ts[i]); }
      return s;
    }
    var ps = tb.getElementsByTagNameNS(NS.a, "p"), out = [];
    for (i = 0; i < ps.length; i++) {
      ts = ps[i].getElementsByTagNameNS(NS.a, "t");
      s = "";
      for (j = 0; j < ts.length; j++) { s += xmlText(ts[j]); }
      out.push(s);
    }
    return out.join("\n");
  }
  /* 某个部件文档里"可搬进幻灯"的占位符(跳过 dt / ftr / sldNum / hdr) */
  function pptxTextPhsOfDoc(doc) {
    var shapes = pptxShapesOf(doc), out = [], i, ph;
    for (i = 0; i < shapes.length; i++) {
      ph = pptxPhOf(shapes[i]);
      if (!ph) { continue; }
      if (!PPTX_TEXT_PH[pptxPhType(ph)]) { continue; }
      out.push(ph);
    }
    return out;
  }
  /* 某部件的 rels → rId 映射 { type, target, mode, node } */
  function relsMapOf(entries, relsPart) {
    var raw = partText(entries, relsPart);
    if (raw == null) { return fail("EOFFICE", "缺少部件 " + relsPart); }
    var par = xmlParse(raw);
    if (!par.ok) { return fail(par.code, relsPart + " 解析失败:" + par.error); }
    var list = par.doc.getElementsByTagNameNS(NS.pr, "Relationship"), map = {}, i, id;
    for (i = 0; i < list.length; i++) {
      id = String(list[i].getAttribute("Id") || "");
      map[id] = {
        id: id,
        type: String(list[i].getAttribute("Type") || ""),
        target: String(list[i].getAttribute("Target") || ""),
        mode: String(list[i].getAttribute("TargetMode") || "")
      };
    }
    return { ok: true, map: map, doc: par.doc, raw: raw };
  }
  /* 幻灯清单(按 p:sldIdLst 顺序) = 索引口径的**唯一来源**(outline / set_text /
     delete / replace_text 都用它;序号 0 起) */
  function pptxSlideList(entries) {
    var p = partDocOf(entries, PART.presentation);
    if (!p.ok) { return p; }
    var rm = relsMapOf(entries, PART.presentationRels);
    if (!rm.ok) { return rm; }
    var ids = p.doc.getElementsByTagNameNS(NS.p, "sldId"), out = [], i, rid, rel, part;
    for (i = 0; i < ids.length; i++) {
      rid = String(ids[i].getAttributeNS(NS.r, "id") || "");
      rel = rm.map[rid];
      if (!rel) { return fail("EOFFICE", PART.presentation + " 的 p:sldId 引用了不存在的 rId " + rid); }
      part = resolvePartName("ppt", rel.target);
      if (!has(entries, part)) { return fail("EOFFICE", "p:sldId 指向的幻灯部件缺失:" + part); }
      out.push({
        i: i, sldId: String(ids[i].getAttribute("id") || ""), rid: rid, part: part,
        relsPart: relsPathOf(part)
      });
    }
    return { ok: true, slides: out, doc: p.doc, decl: p.decl, rels: rm };
  }
  /* 版式清单(母版 p:sldLayoutIdLst → rels → 版式部件):供 outline 展示与寻址 */
  function pptxLayoutsOf(entries) {
    var m = partDocOf(entries, PART.slideMaster);
    if (!m.ok) { return m; }
    var rm = relsMapOf(entries, PART.slideMasterRels);
    if (!rm.ok) { return rm; }
    var ids = m.doc.getElementsByTagNameNS(NS.p, "sldLayoutId"), out = [], i, j;
    for (i = 0; i < ids.length; i++) {
      var rid = String(ids[i].getAttributeNS(NS.r, "id") || "");
      var rel = rm.map[rid];
      if (!rel) { continue; }
      var part = resolvePartName("ppt/slideMasters", rel.target);
      var d = partDocOf(entries, part), name = "", phs = [];
      if (d.ok) {
        var cs = d.doc.getElementsByTagNameNS(NS.p, "cSld");
        if (cs && cs.length) { name = String(cs[0].getAttribute("name") || ""); }
        var shapes = pptxShapesOf(d.doc);
        for (j = 0; j < shapes.length; j++) {
          var ph = pptxPhOf(shapes[j]);
          if (ph) { phs.push({ type: pptxPhType(ph), idx: pptxPhIdx(ph) }); }
        }
      }
      out.push({ i: out.length, rid: rid, part: part, name: name, placeholders: phs });
    }
    return { ok: true, layouts: out };
  }
  /* 某个文档里第一个 ftr 占位符(页脚文本的所在)—— 3 个 ph 里只认 ftr */
  function pptxFtrOf(doc) {
    var shapes = pptxShapesOf(doc), i, ph;
    for (i = 0; i < shapes.length; i++) {
      ph = pptxPhOf(shapes[i]);
      if (ph && pptxPhType(ph) === "ftr") { return { found: true, text: pptxTextOf(shapes[i]) }; }
    }
    return { found: false, text: null };
  }
  /* 一批部件里有没有 <p:hf sldNum="1"/>(批次 G 的写入口;这里只读) */
  function pptxHfFlag(entries, parts, attr) {
    var i, j, d, hfs;
    for (i = 0; i < parts.length; i++) {
      d = partDocOf(entries, parts[i]);
      if (!d.ok) { continue; }
      hfs = d.doc.getElementsByTagNameNS(NS.p, "hf");
      for (j = 0; j < hfs.length; j++) {
        if (String(hfs[j].getAttribute(attr) || "") === "1") { return true; }
      }
    }
    return false;
  }
  /* app.xml 的 <Slides>(docstream 取页数的来源,见 docs/OFFICE-NOTES.md) */
  function setAppSlides(doc, n) {
    var root = doc.documentElement;
    if (!root) { return fail("EOFFICE", PART.app + " 根元素缺失"); }
    var list = root.getElementsByTagNameNS(NS.ep, "Slides");
    var el = (list && list.length) ? list[0] : null;
    if (!el) {
      el = elNs(doc, NS.ep, "Slides");
      /* schema 顺序:Paragraphs → Slides → Notes;有 Notes 就插它前面 */
      var notes = root.getElementsByTagNameNS(NS.ep, "Notes");
      if (notes && notes.length) { root.insertBefore(el, notes[0]); } else { root.appendChild(el); }
    }
    setElemText(doc, el, String(n));
    return { ok: true, value: { slides: n } };
  }
  /* 部件级改写助手(取件 → DOM → 原声明序列化);pptx 的引用同步全走它 */
  function mutatePart(entries, partName, mutate) {
    var d = partDocOf(entries, partName);
    if (!d.ok) { return d; }
    var m = mutate(d.doc);
    if (m && m.ok === false) { return m; }
    var s = xmlSerialize(d.doc, d.decl);
    if (!s.ok) { return s; }
    return { ok: true, text: s.text, value: m };
  }

  /* ---------------- S14 / S17:幻灯构造 ---------------- */
  /* 16:9 版心几何(EMU;与 TPL_PPTX 母版 / 版式同值)—— 只在"自包含文本框"
     (blank 版式 / 版式不可读 / 需自绘)时使用 */
  var PPTX_XF = {
    title: { x: 838200, y: 365125, cx: 10515600, cy: 1325563 },
    body: { x: 838200, y: 1825625, cx: 10515600, cy: 4351338 }
  };
  function pptxXfrm(doc, xf) {
    var f = elA(doc, "xfrm");
    var off = elA(doc, "off"); nAttr(off, "x", xf.x); nAttr(off, "y", xf.y);
    var ext = elA(doc, "ext"); nAttr(ext, "cx", xf.cx); nAttr(ext, "cy", xf.cy);
    f.appendChild(off); f.appendChild(ext);
    return f;
  }
  /* 幻灯上的占位符形状:`p:spPr` 空 ⇒ 几何 / 样式从版式与母版继承(真值同款) */
  function pptxSlideSp(doc, ph, id, name, lines) {
    var sp = elP(doc, "sp");
    var nv = elP(doc, "nvSpPr");
    var cn = elP(doc, "cNvPr"); nAttr(cn, "id", id); nAttr(cn, "name", name);
    nv.appendChild(cn);
    var cs = elP(doc, "cNvSpPr");
    var locks = elA(doc, "spLocks"); nAttr(locks, "noGrp", "1");
    cs.appendChild(locks);
    nv.appendChild(cs);
    var nvPr = elP(doc, "nvPr");
    nvPr.appendChild(pptxPhClone(doc, ph));
    nv.appendChild(nvPr);
    sp.appendChild(nv);
    sp.appendChild(elP(doc, "spPr"));
    sp.appendChild(pptxTxBody(doc, lines));
    return sp;
  }
  /* 自包含文本框(无占位符继承可得时的兜底;blank 版式 / 外部版式不可读) */
  function pptxTextBox(doc, id, name, xf, lines) {
    var sp = elP(doc, "sp");
    var nv = elP(doc, "nvSpPr");
    var cn = elP(doc, "cNvPr"); nAttr(cn, "id", id); nAttr(cn, "name", name);
    nv.appendChild(cn);
    var cs = elP(doc, "cNvSpPr"); nAttr(cs, "txBox", "1");
    nv.appendChild(cs);
    nv.appendChild(elP(doc, "nvPr"));
    sp.appendChild(nv);
    var spPr = elP(doc, "spPr");
    spPr.appendChild(pptxXfrm(doc, xf));
    var geom = elA(doc, "prstGeom"); nAttr(geom, "prst", "rect");
    geom.appendChild(elA(doc, "avLst"));
    spPr.appendChild(geom);
    spPr.appendChild(elA(doc, "noFill"));
    sp.appendChild(spPr);
    sp.appendChild(pptxTxBody(doc, lines));
    return sp;
  }
  /* 一张幻灯的 XML:按版式占位符集合(phs)建形状 —— 有 title / ctrTitle 就填标题,
     余下第一个内容型占位符吃 bullets;phs 为空则自包含文本框 */
  function pptxSlideXml(phs, s) {
    var t = TPL_PPTX.slide;
    var par = xmlParse(t);
    if (!par.ok) { return par; }
    var doc = par.doc, tree = pptxSpTree(doc);
    if (!tree) { return fail("EINTERNAL", "pptx 模板 slide 缺 p:spTree"); }
    var titlePh = null, bodyPh = null, i, tp, id = 2, made = 0;
    for (i = 0; i < phs.length; i++) {
      tp = pptxPhType(phs[i]);
      if (!titlePh && (tp === "title" || tp === "ctrTitle")) { titlePh = phs[i]; continue; }
      if (!bodyPh) { bodyPh = phs[i]; }
    }
    if (titlePh) {
      tree.appendChild(pptxSlideSp(doc, titlePh, id, "Title " + (id - 1),
        s.title == null ? [""] : [s.title]));
      id++; made++;
    }
    if (bodyPh) {
      tree.appendChild(pptxSlideSp(doc, bodyPh, id, "Content " + (id - 1), s.bullets));
      id++; made++;
    }
    if (!phs.length) {
      if (s.title != null) {
        tree.appendChild(pptxTextBox(doc, id, "TextBox " + (id - 1), PPTX_XF.title, [s.title]));
        id++; made++;
      }
      if (s.bullets) {
        tree.appendChild(pptxTextBox(doc, id, "TextBox " + (id - 1), PPTX_XF.body, s.bullets));
        id++; made++;
      }
    }
    var ser = xmlSerialize(doc, xmlDeclOf(t));
    if (!ser.ok) { return ser; }
    return { ok: true, text: ser.text, value: { shapes: made } };
  }
  /* 幻灯 rels:借模板的 Relationship 骨架改 Target(**只有 layout 一条**;批 F/H
     的 chart / image 关系由那些批在末尾追加)—— 用 setAttribute 让转义交给序列化器 */
  function pptxSlideRelsText(tplKey, target) {
    var t = TPL_PPTX[tplKey];
    var par = xmlParse(t);
    if (!par.ok) { return par; }
    var rels = par.doc.getElementsByTagNameNS(NS.pr, "Relationship");
    if (!rels || !rels.length) { return fail("EINTERNAL", "pptx 模板 " + tplKey + " 里没有 Relationship"); }
    rels[0].setAttribute("Type", REL_SLIDE_LAYOUT);
    rels[0].setAttribute("Target", String(target));
    var s = xmlSerialize(par.doc, xmlDeclOf(t));
    if (!s.ok) { return s; }
    return { ok: true, text: s.text };
  }
  /* 幻灯规格归一化(slides[] 的每个元素;layout/title/bullets) */
  function normSlides(sl) {
    var list = [], i, j, it, lay, bullets;
    if (!(sl instanceof Array) || !sl.length) { return fail("EINVAL", "slides 必须是非空数组(至少一张幻灯)"); }
    for (i = 0; i < sl.length; i++) {
      it = sl[i];
      if (!it || typeof it !== "object" || it instanceof Array) { return fail("EINVAL", "slides[" + i + "] 必须是对象"); }
      lay = String(it.layout == null ? "content" : it.layout).toLowerCase();
      if (!has(PPTX_LAYOUT_KEYS, lay)) {
        return fail("EINVAL", "slides[" + i + "].layout 必须是 title / content / blank");
      }
      if (it.title != null && typeof it.title !== "string") {
        return fail("EINVAL", "slides[" + i + "].title 必须是字符串");
      }
      bullets = null;
      if (it.bullets != null) {
        if (!(it.bullets instanceof Array)) { return fail("EINVAL", "slides[" + i + "].bullets 必须是字符串数组"); }
        bullets = [];
        for (j = 0; j < it.bullets.length; j++) {
          bullets.push(String(it.bullets[j] == null ? "" : it.bullets[j]));
        }
      }
      list.push({ layout: lay, title: (it.title == null ? null : String(it.title)), bullets: bullets });
    }
    return { ok: true, list: list };
  }
  /* 包内下一个可用的幻灯号(pptx/slides/slideN.xml 的最大 N + 1) */
  function nextSlideIndex(entries) {
    var max = 0, k, m, v;
    for (k in entries) {
      if (!has(entries, k)) { continue; }
      m = /^ppt\/slides\/slide([0-9]+)\.xml$/.exec(k);
      if (m) { v = parseInt(m[1], 10); if (v > max) { max = v; } }
    }
    return max + 1;
  }
  /* 从 fromDir 到 toPart 的相对 Target(包内路径一律 / 分隔) */
  function relTargetFrom(fromDir, toPart) {
    var a = String(fromDir).split("/"), b = String(toPart).split("/"), i = 0, j;
    var ans = [], bs = [], as = [];
    for (i = 0; i < a.length; i++) { if (a[i] !== "") { as.push(a[i]); } }
    for (i = 0; i < b.length; i++) { if (b[i] !== "") { bs.push(b[i]); } }
    i = 0;
    while (i < as.length && i < bs.length - 1 && as[i] === bs[i]) { i++; }
    for (j = i; j < as.length; j++) { ans.push(".."); }
    for (j = i; j < bs.length; j++) { ans.push(bs[j]); }
    return ans.join("/");
  }
  function headLimitOf(spec) {
    var limit = 80;
    if (spec && spec.head_limit != null) {
      limit = parseInt(spec.head_limit, 10);
      if (isNaN(limit) || limit < 1) { limit = 80; }
    }
    if (limit > 200) { limit = 200; }
    return limit;
  }

  /* ---------------- S14:create(pptx) ---------------- */
  function pptxCreate(spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var sl = spec.slides;
    if (!(sl instanceof Array) || !sl.length) { return fail("EINVAL", "create(pptx)需要 slides:至少一张幻灯"); }
    if (sl.length > LIMITS.maxSlides) {
      return fail("EINVAL", "slides 有 " + sl.length + " 张,超过上限 " + LIMITS.maxSlides);
    }
    var norm = normSlides(sl);
    if (!norm.ok) { return norm; }
    var list = norm.list;
    progress(opts, "build");
    /* 两个版式的"内容型占位符"—— 从模板直接取(create 的包是我们的模板,不查 entries) */
    var l1 = xmlParse(TPL_PPTX.layout1), l2 = xmlParse(TPL_PPTX.layout2);
    if (!l1.ok) { return fail(l1.code, "模板 layout1 解析失败:" + l1.error); }
    if (!l2.ok) { return fail(l2.code, "模板 layout2 解析失败:" + l2.error); }
    var phsOf = { title: pptxTextPhsOfDoc(l1.doc), content: pptxTextPhsOfDoc(l2.doc), blank: [] };
    var parts = {}, added = [], i, key, made, relTxt;
    var fixedKeys = ["rootRels", "theme", "slideMaster", "slideMasterRels", "layout1", "layout2",
      "layout1Rels", "layout2Rels", "presProps", "viewProps", "tableStyles"];
    var names = {
      rootRels: PART.rootRels, theme: PART.theme, slideMaster: PART.slideMaster,
      slideMasterRels: PART.slideMasterRels, layout1: PART.layout1, layout2: PART.layout2,
      layout1Rels: LAYOUT1_RELS, layout2Rels: LAYOUT2_RELS, presProps: PART.presProps,
      viewProps: PART.viewProps, tableStyles: PART.tableStyles
    };
    for (i = 0; i < fixedKeys.length; i++) { parts[names[fixedKeys[i]]] = TPL_PPTX[fixedKeys[i]]; }
    var ctPart = tplPptxOf("contentTypes", function (doc) {
      var root = doc.documentElement, n;
      for (n = 0; n < list.length; n++) {
        var ov = elNs(doc, NS.ct, "Override");
        ov.setAttribute("PartName", "/" + slidePart(n + 1));
        ov.setAttribute("ContentType", CT_PPTX.slide);
        root.appendChild(ov);
      }
      return { ok: true, value: { slides: list.length } };
    });
    if (!ctPart.ok) { return ctPart; }
    parts[PART.contentTypes] = ctPart.text;
    var corePart = tplDocOf("core", function (doc) { return coreApply(doc, spec.props, true); });
    if (!corePart.ok) { return corePart; }
    parts[PART.core] = corePart.text;
    var appPart = tplPptxOf("app", function (doc) { return setAppSlides(doc, list.length); });
    if (!appPart.ok) { return appPart; }
    parts[PART.app] = appPart.text;
    var presPart = tplPptxOf("presentation", function (doc) {
      var lst = doc.getElementsByTagNameNS(NS.p, "sldIdLst"), n;
      if (!lst || !lst.length) { return fail("EINTERNAL", "presentation 模板缺 p:sldIdLst"); }
      for (n = 0; n < list.length; n++) {
        var sid = elP(doc, "sldId");
        nAttr(sid, "id", String(256 + n));
        rAttr(sid, "id", "rId" + (6 + n));
        lst[0].appendChild(sid);
      }
      return { ok: true, value: { slides: list.length } };
    });
    if (!presPart.ok) { return presPart; }
    parts[PART.presentation] = presPart.text;
    var presRels = tplPptxOf("presentationRels", function (doc) {
      var root = doc.documentElement, n;
      for (n = 0; n < list.length; n++) {
        var rel = elNs(doc, NS.pr, "Relationship");
        rel.setAttribute("Id", "rId" + (6 + n));
        rel.setAttribute("Type", REL_SLIDE);
        rel.setAttribute("Target", "slides/slide" + (n + 1) + ".xml");
        root.appendChild(rel);
      }
      return { ok: true, value: { rels: list.length } };
    });
    if (!presRels.ok) { return presRels; }
    parts[PART.presentationRels] = presRels.text;
    var shapeCount = 0;
    for (i = 0; i < list.length; i++) {
      made = pptxSlideXml(phsOf[list[i].layout], list[i]);
      if (!made.ok) { return made; }
      parts[slidePart(i + 1)] = made.text;
      relTxt = pptxSlideRelsText(PPTX_LAYOUT_KEYS[list[i].layout].relKey,
        PPTX_LAYOUT_KEYS[list[i].layout].target);
      if (!relTxt.ok) { return relTxt; }
      parts[slideRelsPart(i + 1)] = relTxt.text;
      added.push(slidePart(i + 1));
      added.push(slideRelsPart(i + 1));
      shapeCount += made.value.shapes;
    }
    var pkg = { entries: {}, order: [], meta: { zip64: false, comment: "" } };
    var fin = finishDraft(pkg, parts, "pptx", true, opts, added);
    if (!fin.ok) { return fin; }
    var steps = ["生成 pptx 固定件 16 件", "幻灯 " + list.length + " 张(每张 slideN.xml + rels)",
      "包结构自检 " + fin.checked.length + " 条通过"];
    return okResult(fin.bytes, "pptx", { slides: list.length, shapes: shapeCount }, [], steps);
  }

  /* ---------------- S15:outline(pptx) ---------------- */
  function pptxOutlineOf(entries, spec) {
    var sl = pptxSlideList(entries);
    if (!sl.ok) { return sl; }
    var lay = pptxLayoutsOf(entries);
    if (!lay.ok) { return lay; }
    var byPart = {}, i, j, k;
    for (i = 0; i < lay.layouts.length; i++) { byPart[lay.layouts[i].part] = lay.layouts[i]; }
    var items = [], shapes = 0;
    for (i = 0; i < sl.slides.length; i++) {
      var d = partDocOf(entries, sl.slides[i].part);
      if (!d.ok) { return d; }
      var els = pptxShapesOf(d.doc), sarr = [];
      for (j = 0; j < els.length; j++) {
        sarr.push({ i: j, kind: pptxKindOf(els[j]), text: brief(pptxTextOf(els[j]), 80, true) });
      }
      shapes += sarr.length;
      var layName = null, layIdx = null;
      var rm = relsMapOf(entries, sl.slides[i].relsPart);
      if (rm.ok) {
        for (k in rm.map) {
          if (!has(rm.map, k)) { continue; }
          if (rm.map[k].type !== REL_SLIDE_LAYOUT) { continue; }
          var lp = resolvePartName("ppt/slides", rm.map[k].target);
          layName = byPart[lp] ? byPart[lp].name : lp;
          layIdx = byPart[lp] ? byPart[lp].i : null;
        }
      }
      items.push({ i: i, layout: layName, layout_i: layIdx, shapes: sarr });
    }
    var limit = headLimitOf(spec);
    /* 页脚 / 编号现状(§13.3 F2:文本落在**每张幻灯自己的** ftr / sldNum 占位符;
       别人的 deck 也可能把文本写在母版 / 版式上 ⇒ 扫描顺序 = 幻灯 → 版式 → 母版,
       取第一个**非空**文本;全空则报第一个找到的(占位符在但没文本))。 */
    var ftr = { found: false, text: null };
    var slideParts = [], i2;
    for (i2 = 0; i2 < sl.slides.length; i2++) { slideParts.push(sl.slides[i2].part); }
    var scan = slideParts.slice();
    for (i2 = 0; i2 < lay.layouts.length; i2++) { scan.push(lay.layouts[i2].part); }
    scan.push(PART.slideMaster);
    for (i2 = 0; i2 < scan.length; i2++) {
      var dd = partDocOf(entries, scan[i2]);
      if (!dd.ok) { continue; }
      var f = pptxFtrOf(dd.doc);
      if (!f.found) { continue; }
      if (!ftr.found) { ftr = f; }
      if (String(f.text || "").length) { ftr = f; break; }
    }
    /* 幻灯号"是否显示" = **幻灯级**有 <a:fld type="slidenum"> 的 sldNum 占位符
       (F2 口径;母版 / 版式自带占位符但幻灯没落下来 = 不显示,真值实测就是这样),
       旧形态(每张幻灯的 <p:hf sldNum="1"/>)一并认 */
    var numOn = pptxHfFlag(entries, slideParts, "sldNum"), j2, pas, phN;
    for (i2 = 0; i2 < slideParts.length && !numOn; i2++) {
      var dn = partDocOf(entries, slideParts[i2]);
      if (!dn.ok) { continue; }
      pas = pptxShapesOf(dn.doc);
      for (j2 = 0; j2 < pas.length; j2++) {
        phN = pptxPhOf(pas[j2]);
        if (phN && pptxPhType(phN) === "sldNum" && pptxHasSldNumField(pas[j2])) { numOn = true; break; }
      }
    }
    return {
      ok: true,
      outline: {
        items: items.slice(0, limit), total: items.length, truncated: items.length > limit,
        head_limit: limit,
        layouts: lay.layouts,
        headerFooter: {
          footer: ftr.found ? ftr.text : null,
          slideNumber: numOn,
          hasFooterPlaceholder: ftr.found
        }
      },
      counts: { slides: sl.slides.length, shapes: shapes }
    };
  }
  function pptxOutline(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var r = pptxOutlineOf(pkg.entries, spec);
    if (!r.ok) { return r; }
    progress(opts, "done");
    /* outline 与 docx 同款:**不产出新字节**(结果里不给 bytes) */
    return {
      ok: true, readOnly: true, mime: MIME.pptx, size: 0,
      outline: r.outline, counts: r.counts, warnings: [],
      steps: ["outline:演示文稿 " + r.outline.total + " 张(返回 " + r.outline.items.length + " 张)"]
    };
  }

  /* ---------------- S16:set_text(pptx) ---------------- */
  function pptxSetText(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var si = spec.slide_index;
    if (typeof si !== "number" || isNaN(si) || si < 0 || Math.floor(si) !== si) {
      return fail("EINVAL", "slide_index 必须是不小于 0 的整数(0 起,取自 outline)");
    }
    if (!(spec.shape_text instanceof Array)) { return fail("EINVAL", "shape_text 必须是字符串数组"); }
    var shi = (spec.shape_index == null) ? 0 : spec.shape_index;
    if (typeof shi !== "number" || isNaN(shi) || shi < 0 || Math.floor(shi) !== shi) {
      return fail("EINVAL", "shape_index 必须是不小于 0 的整数(省略 = 该页第 0 个形状)");
    }
    var lines = [], i;
    for (i = 0; i < spec.shape_text.length; i++) {
      lines.push(String(spec.shape_text[i] == null ? "" : spec.shape_text[i]));
    }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var list = pptxSlideList(pkg.entries);
    if (!list.ok) { return list; }
    if (si >= list.slides.length) {
      return fail("EINVAL", "slide_index " + si + " 越界(共 " + list.slides.length + " 张)");
    }
    var ent = list.slides[si];
    progress(opts, "build");
    var d = partDocOf(pkg.entries, ent.part);
    if (!d.ok) { return d; }
    var els = pptxShapesOf(d.doc);
    if (shi >= els.length) {
      return fail("EINVAL", "shape_index " + shi + " 越界(第 " + si + " 张共 " + els.length + " 个形状)");
    }
    var el = els[shi];
    if (el.localName !== "sp") {
      return fail("EINVAL", "第 " + si + " 张的第 " + shi + " 个形状是 p:" + el.localName
        + ",set_text 只能改文本框(p:sp)");
    }
    var tb = pptxTxBodyOf(el);
    if (!tb) { return fail("EINVAL", "第 " + si + " 张的第 " + shi + " 个形状没有 p:txBody"); }
    var kids = tb.childNodes, n;
    for (i = kids.length - 1; i >= 0; i--) {
      n = kids[i];
      if (n.nodeType === 1 && n.namespaceURI === NS.a && n.localName === "p") { tb.removeChild(n); }
    }
    var arr = lines.length ? lines : [""];
    for (i = 0; i < arr.length; i++) { tb.appendChild(pptxPara(d.doc, arr[i])); }
    var s = xmlSerialize(d.doc, d.decl);
    if (!s.ok) { return s; }
    var writes = {};
    writes[ent.part] = s.text;
    var fin = finishDraft(pkg, writes, "pptx", false, opts);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "pptx", { shapes: 1 },
      [], ["第 " + si + " 张第 " + shi + " 个形状:段落整体替换为 " + arr.length + " 段"]);
  }

  /* ---------------- S17:add_slide(pptx) ---------------- */
  function pptxAddSlide(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var sl = spec.slides;
    if (!(sl instanceof Array) || !sl.length) { return fail("EINVAL", "add_slide 需要 slides(单元素数组)"); }
    var norm = normSlides([sl[0]]);
    if (!norm.ok) { return norm; }
    var one = norm.list[0], warns = [], i, k, m;
    if (sl.length > 1) { warns.push("add_slide 只取 slides[0],已忽略其余的 " + (sl.length - 1) + " 张"); }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var list = pptxSlideList(pkg.entries);
    if (!list.ok) { return list; }
    progress(opts, "build");
    /* 目标版式:**自产 pptx**(判据 = docProps/app.xml 的 Application == "AzusaAI WebUI")
       按 layout 参数取我们的两个版式;外部 pptx 沿用"最后一页的版式"(与 PowerPoint
       新建幻灯一致),此时 layout 参数不生效并出 warning */
    var want = PPTX_LAYOUT_KEYS[one.layout];
    var layPart = null, layKey = "layout2Rels", layTarget = want.target;
    if (pptxIsOwnDeck(pkg.entries) && has(pkg.entries, want.part)) {
      layPart = want.part;
      layKey = want.relKey;
    } else if (list.slides.length) {
      var last = list.slides[list.slides.length - 1];
      var rm = relsMapOf(pkg.entries, last.relsPart);
      if (rm.ok) {
        for (k in rm.map) {
          if (!has(rm.map, k) || rm.map[k].type !== REL_SLIDE_LAYOUT) { continue; }
          layTarget = rm.map[k].target;
          layPart = resolvePartName("ppt/slides", layTarget);
          break;
        }
      }
      if (layPart) {
        warns.push("外部 pptx:add_slide 沿用最后一页的版式(" + layPart + "),layout 参数仅对自产 pptx 生效");
      }
    }
    if (!layPart) {
      var lay = pptxLayoutsOf(pkg.entries);
      if (lay.ok && lay.layouts.length) { layPart = lay.layouts[0].part; }
    }
    if (layPart) { layTarget = relTargetFrom("ppt/slides", layPart); }
    var phs = [];
    if (layPart) {
      var ld = partDocOf(pkg.entries, layPart);
      if (ld.ok) { phs = pptxTextPhsOfDoc(ld.doc); }
      else { warns.push("版式 " + layPart + " 不可读,新页退化为自包含文本框"); }
    } else {
      warns.push("包内找不到可用版式,新页退化为自包含文本框");
    }
    var n = nextSlideIndex(pkg.entries);
    var made = pptxSlideXml(phs, one);
    if (!made.ok) { return made; }
    var relTxt = pptxSlideRelsText(layKey, layTarget);
    if (!relTxt.ok) { return relTxt; }
    var writes = {}, steps = [];
    writes[slidePart(n)] = made.text;
    writes[slideRelsPart(n)] = relTxt.text;
    /* 同事务 4 处引用(方案 §3 S17)+ app.xml 的 <Slides>(§3 S18 的三处之一) */
    var ct = ctEnsureOverride(pkg.entries, slidePart(n), CT_PPTX.slide);
    if (!ct.ok) { return ct; }
    if (ct.text != null) { writes[PART.contentTypes] = ct.text; }
    var maxId = 255, maxRid = 0;
    for (i = 0; i < list.slides.length; i++) {
      var v = parseInt(list.slides[i].sldId, 10);
      if (!isNaN(v) && v > maxId) { maxId = v; }
    }
    for (k in list.rels.map) {
      if (!has(list.rels.map, k)) { continue; }
      m = /^rId([0-9]+)$/.exec(k);
      if (m && parseInt(m[1], 10) > maxRid) { maxRid = parseInt(m[1], 10); }
    }
    var newId = maxId + 1, newRid = "rId" + (maxRid + 1);
    var pw = mutatePart(pkg.entries, PART.presentation, function (doc) {
      var lst = doc.getElementsByTagNameNS(NS.p, "sldIdLst");
      if (!lst || !lst.length) { return fail("EOFFICE", PART.presentation + " 缺少 p:sldIdLst"); }
      var sid = elP(doc, "sldId");
      nAttr(sid, "id", String(newId));
      rAttr(sid, "id", newRid);
      lst[0].appendChild(sid);
      return { ok: true, value: { id: newId, rid: newRid } };
    });
    if (!pw.ok) { return pw; }
    writes[PART.presentation] = pw.text;
    var rw = mutatePart(pkg.entries, PART.presentationRels, function (doc) {
      var root = doc.documentElement;
      var rel = elNs(doc, NS.pr, "Relationship");
      rel.setAttribute("Id", newRid);
      rel.setAttribute("Type", REL_SLIDE);
      rel.setAttribute("Target", "slides/slide" + n + ".xml");
      root.appendChild(rel);
      return { ok: true, value: { rid: newRid } };
    });
    if (!rw.ok) { return rw; }
    writes[PART.presentationRels] = rw.text;
    var total = list.slides.length + 1;
    var aw = appSlidesSet(pkg.entries, total, warns);
    if (aw.text != null) { writes[PART.app] = aw.text; }
    steps.push("追加 " + slidePart(n) + " + rels(版式 " + layTarget + ")");
    steps.push("同步 4 处引用:Content_Types Override / p:sldId(id=" + newId + " " + newRid
      + ") / presentation.xml.rels / docProps/app.xml <Slides>=" + total);
    var fin = finishDraft(pkg, writes, "pptx", false, opts, [slidePart(n), slideRelsPart(n)]);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "pptx", { slides: total, added: 1 }, warns, steps);
  }

  /* app.xml 的 <Slides> 同步(缺 app.xml 只登记 warning,不新建部件) */
  function appSlidesSet(entries, n, warns) {
    if (!has(entries, PART.app)) {
      warns.push("包内没有 " + PART.app + ",未更新 <Slides>(实际 " + n + " 张)");
      return { text: null };
    }
    var w = mutatePart(entries, PART.app, function (doc) { return setAppSlides(doc, n); });
    if (!w.ok) { warns.push("更新 <Slides> 失败:" + w.error); return { text: null }; }
    return { text: w.text };
  }

  /* ---------------- S17:delete(pptx) ---------------- */
  function pptxDelete(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var si = spec.slide_index;
    if (typeof si !== "number" || isNaN(si) || si < 0 || Math.floor(si) !== si) {
      return fail("EINVAL", "slide_index 必须是不小于 0 的整数(0 起,取自 outline)");
    }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var list = pptxSlideList(pkg.entries);
    if (!list.ok) { return list; }
    if (si >= list.slides.length) {
      return fail("EINVAL", "slide_index " + si + " 越界(共 " + list.slides.length + " 张)");
    }
    var ent = list.slides[si], warns = [], steps = [], i, k;
    progress(opts, "build");
    var writes = {};
    /* 删两件:幻灯部件 + 它的 rels(名由 relsPathOf 派生,与写侧同源)。
       包的 rels 件不存在时(外部包的少见形态)如实给 warning —— 不留静默残留 */
    if (!has(pkg.entries, ent.relsPart)) {
      warns.push("第 " + si + " 张幻灯没有关系部件 " + ent.relsPart + ",本次只删幻灯部件");
    }
    writes[ent.part] = null;
    writes[ent.relsPart] = null;
    var ct = ctRemoveOverride(pkg.entries, ent.part);
    if (!ct.ok) { return ct; }
    if (ct.changed) { writes[PART.contentTypes] = ct.text; }
    var pw = mutatePart(pkg.entries, PART.presentation, function (doc) {
      var ids = doc.getElementsByTagNameNS(NS.p, "sldId"), n;
      for (n = 0; n < ids.length; n++) {
        if (String(ids[n].getAttribute("id") || "") !== ent.sldId) { continue; }
        if (String(ids[n].getAttributeNS(NS.r, "id") || "") !== ent.rid) { continue; }
        ids[n].parentNode.removeChild(ids[n]);
        return { ok: true, value: { removed: 1 } };
      }
      return fail("EOFFICE", "在 p:sldIdLst 里找不到要删的条目(id=" + ent.sldId + " " + ent.rid + ")");
    });
    if (!pw.ok) { return pw; }
    writes[PART.presentation] = pw.text;
    var rw = mutatePart(pkg.entries, PART.presentationRels, function (doc) {
      var rels = doc.getElementsByTagNameNS(NS.pr, "Relationship"), n;
      for (n = 0; n < rels.length; n++) {
        if (String(rels[n].getAttribute("Id") || "") !== ent.rid) { continue; }
        if (String(rels[n].getAttribute("Type") || "") !== REL_SLIDE) { continue; }
        rels[n].parentNode.removeChild(rels[n]);
        return { ok: true, value: { removed: 1 } };
      }
      return fail("EOFFICE", "在 " + PART.presentationRels + " 里找不到要删的关系 " + ent.rid);
    });
    if (!rw.ok) { return rw; }
    writes[PART.presentationRels] = rw.text;
    var total = list.slides.length - 1;
    var aw = appSlidesSet(pkg.entries, total, warns);
    if (aw.text != null) { writes[PART.app] = aw.text; }
    steps.push("删除 " + ent.part + " + 其 rels");
    steps.push("同步 4 处引用:Content_Types Override / p:sldId(" + ent.sldId + " " + ent.rid
      + ") / presentation.xml.rels / docProps/app.xml <Slides>=" + total);
    var fin = finishDraft(pkg, writes, "pptx", false, opts);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "pptx", { slides: total, deleted: 1 }, warns, steps);
  }

  /* ---------------- S17:replace_text(pptx,两阶段) ---------------- */
  /* 阶段①单 a:t 精确命中(格式零损失)→ 阶段②某段的 a:t 拼接串命中则整段重写
     (写回首 run、清空其余 a:t,保留 run 节点与 rPr)。重写范围 = **单个 p:sp**
     (不跨形状、不跨段落),与方案 §3 S17「段落级重写只针对单个 p:sp」一致。 */
  function pptxReplaceText(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    if (typeof spec.old_string !== "string" || !spec.old_string.length) {
      return fail("EINVAL", "old_string 必须是非空字符串");
    }
    var oldS = spec.old_string;
    var newS = (spec.new_string == null) ? "" : String(spec.new_string);
    var all = (spec.replace_all !== false);
    var si = spec.slide_index, shi = spec.shape_index, i, j, k, q;
    if (si != null && (typeof si !== "number" || isNaN(si) || si < 0 || Math.floor(si) !== si)) {
      return fail("EINVAL", "slide_index 必须是不小于 0 的整数");
    }
    if (shi != null && (typeof shi !== "number" || isNaN(shi) || shi < 0 || Math.floor(shi) !== shi)) {
      return fail("EINVAL", "shape_index 必须是不小于 0 的整数");
    }
    if (shi != null && si == null) {
      return fail("EINVAL", "replace_text 的 shape_index 需要与 slide_index 一起给出(限定到某一张)");
    }
    progress(opts, "parse");
    var u8 = asBytes(bytes);
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var list = pptxSlideList(pkg.entries);
    if (!list.ok) { return list; }
    var scope = [];
    if (si == null) {
      for (i = 0; i < list.slides.length; i++) { scope.push(i); }
    } else {
      if (si >= list.slides.length) {
        return fail("EINVAL", "slide_index " + si + " 越界(共 " + list.slides.length + " 张)");
      }
      scope.push(si);
    }
    progress(opts, "build");
    var units = [];
    for (i = 0; i < scope.length; i++) {
      var ent = list.slides[scope[i]];
      var d = partDocOf(pkg.entries, ent.part);
      if (!d.ok) { return d; }
      var allShapes = pptxShapesOf(d.doc), picked = [];
      if (shi == null) {
        picked = allShapes;
      } else {
        if (shi >= allShapes.length) {
          return fail("EINVAL", "shape_index " + shi + " 越界(第 " + scope[i] + " 张共 " + allShapes.length + " 个形状)");
        }
        picked = [allShapes[shi]];
      }
      units.push({ i: scope[i], part: ent.part, doc: d.doc, decl: d.decl, shapes: picked, all: allShapes });
    }
    /* 阶段① */
    var hits = 0, occ, cur;
    for (i = 0; i < units.length; i++) {
      var nd = units[i].doc.getElementsByTagNameNS(NS.a, "t");
      for (j = 0; j < nd.length; j++) {
        if (!inShapes(units[i].doc, units[i].shapes, nd[j])) { continue; }
        occ = occCount(xmlText(nd[j]), oldS);
        if (occ > 0) { hits++; }
      }
    }
    if (hits > 0) {
      var total = 0;
      for (i = 0; i < units.length; i++) {
        var ns2 = units[i].doc.getElementsByTagNameNS(NS.a, "t");
        for (j = 0; j < ns2.length; j++) {
          if (!inShapes(units[i].doc, units[i].shapes, ns2[j])) { continue; }
          total += occCount(xmlText(ns2[j]), oldS);
        }
      }
      if (!all && total > 1) {
        return fail("EINVAL", "old_string 命中 " + total + " 处,replace_all=false 要求唯一命中");
      }
      var left = all ? -1 : 1, replaced = 0;
      for (i = 0; i < units.length; i++) {
        var ns3 = units[i].doc.getElementsByTagNameNS(NS.a, "t");
        for (j = 0; j < ns3.length; j++) {
          if (!inShapes(units[i].doc, units[i].shapes, ns3[j])) { continue; }
          cur = xmlText(ns3[j]);
          if (cur.indexOf(oldS) < 0) { continue; }
          if (all) {
            replaced += occCount(cur, oldS);
            setElemText(units[i].doc, ns3[j], cur.split(oldS).join(newS));
            units[i].changed = true;
          } else if (left > 0) {
            var at = cur.indexOf(oldS);
            setElemText(units[i].doc, ns3[j], cur.slice(0, at) + newS + cur.slice(at + oldS.length));
            replaced++;
            left--;
            units[i].changed = true;
          }
        }
      }
      var w1 = {}, steps1 = ["单 a:t 精确命中替换 " + replaced + " 处"];
      /* 只序列化**真的改过**的部件(没命中的幻灯逐字节不动 —— "不碰就不变") */
      for (i = 0; i < units.length; i++) {
        if (!units[i].changed) { continue; }
        var s1 = xmlSerialize(units[i].doc, units[i].decl);
        if (!s1.ok) { return s1; }
        w1[units[i].part] = s1.text;
      }
      var fin1 = finishDraft(pkg, w1, "pptx", false, opts);
      if (!fin1.ok) { return fin1; }
      return okResult(fin1.bytes, "pptx", { replaced: replaced }, [], steps1);
    }
    /* 阶段②:段落级(单个 p:sp 内) */
    var plan = [], warnings = [], tb, ps, ts, joined;
    for (i = 0; i < units.length; i++) {
      for (j = 0; j < units[i].shapes.length; j++) {
        tb = pptxTxBodyOf(units[i].shapes[j]);
        if (!tb) { continue; }
        ps = tb.getElementsByTagNameNS(NS.a, "p");
        for (k = 0; k < ps.length; k++) {
          ts = ps[k].getElementsByTagNameNS(NS.a, "t");
          if (!ts.length) { continue; }
          joined = "";
          for (q = 0; q < ts.length; q++) { joined += xmlText(ts[q]); }
          occ = occCount(joined, oldS);
          if (occ > 0) {
            plan.push({
              unit: units[i], si: units[i].i, shape: j, para: k, ts: ts, text: joined, occ: occ
            });
          }
        }
      }
    }
    var total2 = 0;
    for (i = 0; i < plan.length; i++) { total2 += plan[i].occ; }
    if (total2 > 0 && !all && total2 > 1) {
      return fail("EINVAL", "old_string 命中 " + total2 + " 处(跨 run),replace_all=false 要求唯一命中");
    }
    var left2 = all ? -1 : 1, replaced2 = 0, it, next, at2;
    for (i = 0; i < plan.length; i++) {
      it = plan[i];
      next = it.text;
      if (all) {
        next = next.split(oldS).join(newS);
        replaced2 += it.occ;
      } else if (left2 > 0) {
        at2 = next.indexOf(oldS);
        next = next.slice(0, at2) + newS + next.slice(at2 + oldS.length);
        replaced2++;
        left2--;
      }
      setElemText(it.unit.doc, it.ts[0], next);
      for (q = 1; q < it.ts.length; q++) { setElemText(it.unit.doc, it.ts[q], ""); }
      it.unit.changed = true;
      warnings.push("第 " + it.si + " 张幻灯第 " + it.shape + " 个形状第 " + it.para
        + " 段命中跨 run,已按整段重写,该段 run 级格式可能变化");
    }
    if (replaced2 === 0) {
      warnings.push("未找到 old_string,演示文稿未改动");
      return {
        ok: true, bytes: u8, mime: MIME.pptx, size: u8.length, unchanged: true,
        counts: { replaced: 0 }, warnings: warnings,
        steps: ["单 a:t 与段落级命中均为 0 处"]
      };
    }
    var w2 = {};
    for (i = 0; i < units.length; i++) {
      if (!units[i].changed) { continue; }
      var s2 = xmlSerialize(units[i].doc, units[i].decl);
      if (!s2.ok) { return s2; }
      w2[units[i].part] = s2.text;
    }
    var fin2 = finishDraft(pkg, w2, "pptx", false, opts);
    if (!fin2.ok) { return fin2; }
    return okResult(fin2.bytes, "pptx", { replaced: replaced2 }, warnings,
      ["单 a:t 精确命中 0 处,跨 run 段落级重写 " + plan.length + " 段"]);
  }
  /* node 是否落在给定形状集合内(shapes 为形状元素,直接用祖先链判)—— 避免逐个
     contains 的高阶写法(ES5 纪律) */
  function inShapes(doc, shapes, node) {
    var p = node, i;
    while (p) {
      for (i = 0; i < shapes.length; i++) { if (shapes[i] === p) { return true; } }
      p = p.parentNode;
    }
    return false;
  }

  /* ---------------- S18:set_properties(pptx) ---------------- */
  function pptxSetProperties(bytes, spec, opts) { return propsSetCore(bytes, "pptx", spec, opts); }

  /* ============================================================
     批次 F:图表 + 内嵌工作簿(方案 §3 的 S28–S32 / §12 全文)
     分层:S28 xlsxMinimal(手写最小工作簿)→ S29 chartXml(chartSpace,DOM 构造)→
           S30 pptx 宿主 p:graphicFrame + 6 处同步 → S31 docx 宿主(条件项)→
           S32 校验三段(⑤ 图表链 / ⑥ 内嵌工作簿 / ⑦ 宿主要素名白名单)
     口径与真值(只读实读;§12.2 / §12.3 / §12.6 / §12.7):
       · 网格 = A1 空 / B1 = 系列1 名 / C1 = 系列2 名 …;A2.. = 分类名;B2.. = 系列1 值 …
         ⇒ c:f 的引用必须与网格坐标一致(§12.7「值一致性」/ T13)
       · 宿主的**父元素必须是 a:graphic**(DrawingML)—— OOXML 里不存在 p:graphic
         (§12.3 v1.3 / P1-2);S32 第 ⑦ 条在**写盘前**拦下(PowerPoint 不认 = 打不开)
       · c:ser 的 c:spPr 位置属 `?` 推断(本机 7 份真值的 c:ser 全无 c:spPr)
         ⇒ 正例到手前**一律不写** c:spPr(§12.5 注 / §12.6 第 15 条)
       · c:externalData 必须在 c:chart **之后**;两个 c:axId 与 c:catAx / c:valAx 的
         @axId 值必须相等;c:plotVisOnly **不写**,c:dispBlanksAs val="gap" **写**
         —— 与真值 gt/chart1.xml 逐字同形(批次 F-2 更正:原头注写"两个都不写",
         与代码不符、属注释错;方案 §12.6 第 2 条的同一句话待 Architect 按实现改准)
       · 文本一律 createTextNode(DrawingML / SpreadsheetML 的文本元素没有 w:t 的
         xml:space 语义 ⇒ 不走 setElemText,产品 XML 里也不多写属性)
       · c:varyColors 显式写全(§12.6 第 10 / 16 条):饼取 1,柱 / 折线取 0 —— 缺省为
         true,省略会让单系列柱 / 折线被 PowerPoint 渲染成"逐点着色 + 图例列分类名"
     ============================================================ */
  var CT_CHART = 'application/vnd.openxmlformats-officedocument.drawingml.chart+xml';
  var REL_CHART = REL_DOC_RELS + 'chart';
  var REL_PACKAGE = REL_DOC_RELS + 'package';
  var REL_WORKSHEET = REL_DOC_RELS + 'worksheet';
  var URI_CHART = 'http://schemas.openxmlformats.org/drawingml/2006/chart';
  var URI_TABLE = 'http://schemas.openxmlformats.org/drawingml/2006/table';
  var URI_PICTURE = 'http://schemas.openxmlformats.org/drawingml/2006/picture';
  var CT_RELS_XML = 'application/vnd.openxmlformats-package.relationships+xml';
  var CT_XML_DEFAULT = 'application/xml';
  /* 内嵌工作簿(SpreadsheetML) */
  var XLSX_NS = 'http://schemas.openxmlformats.org/spreadsheetml/2006/main';
  var CT_SHEET_MAIN = 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml';
  var CT_SHEET = 'application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml';
  var SHEET_NAME = 'Sheet1';                     /* §12.6 第 8 条:表名固定,与 c:f 一致 */
  var CHART_TYPES = { bar: 1, line: 1, pie: 1 };
  /* §12.6 第 5 条:两个互不相等的**大负数**(与真值同值,便于真值对拍) */
  var CHART_AXIS_CAT = -2068027336;
  var CHART_AXIS_VAL = -2113994440;
  /* §12.3:16:9 版心(左右各留 1 英寸、垂直居中)= 12192000 - 2x914400 x 3657600 */
  var PPTX_CHART_XF = { x: 914400, y: 1600200, cx: 10363200, cy: 3657600 };
  var DOCX_CHART_CX = 5486400, DOCX_CHART_CY = 3200400;   /* 6 x 3.5 英寸(§12.3 docx 骨架) */

  /* ---------------- F1:通用小助手 ---------------- */
  /* 列名(0 起):0→A … 25→Z、26→AA(§12.7 的 dimension 与 c:f 共用同一算法) */
  function colName(i) {
    var n = i + 1, s = "";
    while (n > 0) {
      s = String.fromCharCode(65 + ((n - 1) % 26)) + s;
      n = Math.floor((n - 1) / 26);
    }
    return s;
  }
  function cellAddr(col, row) { return colName(col) + row; }
  /* 十进制字面量:有限数一律**不用科学计数法**(§12.6 第 13 条) */
  function numStr(n) {
    if (typeof n !== "number" || !isFinite(n)) {
      throw errOf("EINVAL", "数值必须是有限数(§12.6 第 13 条):" + String(n));
    }
    var s = String(n);
    if (s.indexOf("e") < 0 && s.indexOf("E") < 0) { return s; }
    var m = /^(-?)([0-9]+)(?:\.([0-9]+))?[eE]([+-]?[0-9]+)$/.exec(s);
    if (!m) { return s; }
    var digits = m[2] + (m[3] || ""), at = m[2].length + parseInt(m[4], 10), pad;
    if (at <= 0) { pad = new Array(-at + 1).join("0"); return m[1] + "0." + pad + digits; }
    if (at >= digits.length) { pad = new Array(at - digits.length + 1).join("0"); return m[1] + digits + pad; }
    return m[1] + digits.slice(0, at) + "." + digits.slice(at);
  }
  /* 新建部件一律从"空根"起:<qname xmlns=… />,**不拼业务 XML 字符串**
     (chartSpace / 工作簿五件 / 图表 rels 都经它——值与文本全部走 DOM) */
  function xmlSkeleton(qname, decls) {
    var s = "<" + qname, i;
    for (i = 0; i < decls.length; i++) { s += " " + decls[i][0] + '="' + decls[i][1] + '"'; }
    s += "/>";
    var par = xmlParse(XML_DECL_STD + s);
    if (!par.ok) { return par; }
    return { ok: true, doc: par.doc };
  }
  function addText(el, s) { el.appendChild(el.ownerDocument.createTextNode(String(s == null ? "" : s))); return el; }
  /* chartSpace 的构造助手(批次 F 专属;elP / elA 在批次 C 已定义) */
  function elC(doc, name) { return doc.createElementNS(NS.c, "c:" + name); }
  function cVal(doc, name, v) { var e = elC(doc, name); nAttr(e, "val", String(v)); return e; }
  /* 空根 → 建树 → 序列化(新部件的统一出口;声明用标准声明) */
  function simplePart(qname, decls, build) {
    var sk = xmlSkeleton(qname, decls);
    if (!sk.ok) { return sk; }
    var root = sk.doc.documentElement;
    if (build) {
      var m = build(sk.doc, root);
      if (m && m.ok === false) { return m; }
    }
    var s = xmlSerialize(sk.doc);
    if (!s.ok) { return s; }
    return { ok: true, text: s.text };
  }
  function addRel(doc, root, id, type, target) {
    var rel = elNs(doc, NS.pr, "Relationship");
    rel.setAttribute("Id", id);
    rel.setAttribute("Type", type);
    rel.setAttribute("Target", target);
    root.appendChild(rel);
    return rel;
  }
  /* 把某条关系追加进**已有**部件的 rels(既有条目与 Id 一律不动,§12.2⑤) */
  function relAppend(doc, id, type, target) {
    return { ok: true, value: { id: id, rel: addRel(doc, doc.documentElement, id, type, target) } };
  }
  function maxRidOf(map) {
    var max = 0, k, m;
    for (k in map) {
      if (!has(map, k)) { continue; }
      m = /^rId([0-9]+)$/.exec(k);
      if (m && parseInt(m[1], 10) > max) { max = parseInt(m[1], 10); }
    }
    return max;
  }

  /* ---------------- S28:内嵌工作簿(§12.7 路线 A:手写 5 件) ----------------
     grid = 行数组,行 = 单元格数组;null / undefined = 该格不写(真值 A1 就是空的),
     字符串 ⇒ t="inlineStr" + <is><t>(免 sharedStrings 部件),数值 ⇒ <v>。
     部件:[Content_Types].xml / _rels/.rels / xl/workbook.xml /
           xl/_rels/workbook.xml.rels / xl/worksheets/sheet1.xml(可选 docProps/* 略过)。
     路线判据(§12.7 / P-B):本函数 = 路线 A;XLSX.read 或 Excel COM 任一失败 ⇒ 切路线 B
     (sheetjsOf + XLSX.write),切换是**探针结论**不是默认路径(本批判定读数见 progress)。 */
  function xlsxMinimal(grid) {
    if (!(grid instanceof Array) || !grid.length) { return fail("EINVAL", "内嵌工作簿网格为空"); }
    var i, j, row, v, maxCol = 0;
    for (i = 0; i < grid.length; i++) {
      row = grid[i];
      if (row && row.length > maxCol) { maxCol = row.length; }
    }
    if (!maxCol) { return fail("EINVAL", "内嵌工作簿网格为空"); }
    var sheets = simplePart("worksheet", [["xmlns", XLSX_NS], ["xmlns:r", NS.r]], function (doc, root) {
      var dim = elNs(doc, XLSX_NS, "dimension");
      dim.setAttribute("ref", cellAddr(0, 1) + ":" + cellAddr(maxCol - 1, grid.length));
      root.appendChild(dim);
      var data = elNs(doc, XLSX_NS, "sheetData");
      root.appendChild(data);
      for (var r = 0; r < grid.length; r++) {
        var cells = grid[r] || [];
        var rEl = elNs(doc, XLSX_NS, "row");
        rEl.setAttribute("r", String(r + 1));
        rEl.setAttribute("spans", "1:" + maxCol);
        for (var c2 = 0; c2 < cells.length; c2++) {
          if (cells[c2] == null) { continue; }
          var cEl = elNs(doc, XLSX_NS, "c");
          cEl.setAttribute("r", cellAddr(c2, r + 1));
          if (typeof cells[c2] === "number") {
            cEl.appendChild(addText(elNs(doc, XLSX_NS, "v"), numStr(cells[c2])));
          } else {
            cEl.setAttribute("t", "inlineStr");
            var is = elNs(doc, XLSX_NS, "is");
            is.appendChild(addText(elNs(doc, XLSX_NS, "t"), cells[c2]));
            cEl.appendChild(is);
          }
          rEl.appendChild(cEl);
        }
        data.appendChild(rEl);
      }
      return null;
    });
    if (!sheets.ok) { return sheets; }
    var ct = simplePart("Types", [["xmlns", NS.ct]], function (doc, root) {
      var d1 = elNs(doc, NS.ct, "Default");
      d1.setAttribute("Extension", "rels"); d1.setAttribute("ContentType", CT_RELS_XML);
      root.appendChild(d1);
      var d2 = elNs(doc, NS.ct, "Default");
      d2.setAttribute("Extension", "xml"); d2.setAttribute("ContentType", CT_XML_DEFAULT);
      root.appendChild(d2);
      var o1 = elNs(doc, NS.ct, "Override");
      o1.setAttribute("PartName", "/xl/workbook.xml"); o1.setAttribute("ContentType", CT_SHEET_MAIN);
      root.appendChild(o1);
      var o2 = elNs(doc, NS.ct, "Override");
      o2.setAttribute("PartName", "/xl/worksheets/sheet1.xml"); o2.setAttribute("ContentType", CT_SHEET);
      root.appendChild(o2);
      return null;
    });
    if (!ct.ok) { return ct; }
    var rootRels = simplePart("Relationships", [["xmlns", NS.pr]], function (doc, root) {
      addRel(doc, root, "rId1", REL_OFFICE_DOC, "xl/workbook.xml");
      return null;
    });
    if (!rootRels.ok) { return rootRels; }
    var wb = simplePart("workbook", [["xmlns", XLSX_NS], ["xmlns:r", NS.r]], function (doc, root) {
      var shs = elNs(doc, XLSX_NS, "sheets");
      var sh = elNs(doc, XLSX_NS, "sheet");
      sh.setAttribute("name", SHEET_NAME); sh.setAttribute("sheetId", "1");
      attrNS(sh, NS.r, "r:id", "rId1");
      shs.appendChild(sh); root.appendChild(shs);
      var cp = elNs(doc, XLSX_NS, "calcPr");
      cp.setAttribute("calcId", "124519"); cp.setAttribute("fullCalcOnLoad", "1");
      root.appendChild(cp);
      return null;
    });
    if (!wb.ok) { return wb; }
    var wbRels = simplePart("Relationships", [["xmlns", NS.pr]], function (doc, root) {
      addRel(doc, root, "rId1", REL_WORKSHEET, "worksheets/sheet1.xml");
      return null;
    });
    if (!wbRels.ok) { return wbRels; }
    var entries = {}, order = [];
    entries[PART.contentTypes] = ct.text;
    entries["_rels/.rels"] = rootRels.text;
    entries["xl/workbook.xml"] = wb.text;
    entries["xl/_rels/workbook.xml.rels"] = wbRels.text;
    entries["xl/worksheets/sheet1.xml"] = sheets.text;
    var k;
    for (k in entries) { if (has(entries, k)) { order.push(k); } }
    var z = zipBuild(entries, order);
    if (!z.ok) { return z; }
    return { ok: true, bytes: z.bytes, size: z.bytes.length, parts: z.order };
  }

  /* ---------------- S29:chartSpace(§12.5 三类差异 / §12.6 顺序即正确性) ---------------- */
  function chartSerD(doc, s, i, spec) {
    var ser = elC(doc, "ser");
    ser.appendChild(cVal(doc, "idx", i));
    ser.appendChild(cVal(doc, "order", i));
    /* c:spPr 一律不写(§12.5 注 / §12.6 第 15 条:本机无位置正例,写错即顺序违规) */
    var tx = elC(doc, "tx"), nref = elC(doc, "strRef");
    nref.appendChild(addText(elC(doc, "f"), spec.nameRefs[i]));
    nref.appendChild(chartCacheD(doc, "str", [s.name]));
    tx.appendChild(nref);
    ser.appendChild(tx);
    var cat = elC(doc, "cat"), cref = elC(doc, "strRef");
    cref.appendChild(addText(elC(doc, "f"), spec.catRef));
    cref.appendChild(chartCacheD(doc, "str", spec.categories));
    cat.appendChild(cref);
    ser.appendChild(cat);
    var val = elC(doc, "val"), vref = elC(doc, "numRef");
    vref.appendChild(addText(elC(doc, "f"), spec.valRefs[i]));
    vref.appendChild(chartCacheD(doc, "num", s.values));
    val.appendChild(vref);
    ser.appendChild(val);
    return ser;
  }
  /* c:strCache / c:numCache:ptCount == pt 个数、pt/@idx 从 0 连续;
     numCache 带 <c:formatCode>General</c:formatCode>(**元素文本**形式,§12.6 第 7 条) */
  function chartCacheD(doc, kind, values) {
    var isNum = (kind === "num");
    var cache = elC(doc, isNum ? "numCache" : "strCache");
    if (isNum) { cache.appendChild(addText(elC(doc, "formatCode"), "General")); }
    cache.appendChild(cVal(doc, "ptCount", values.length));
    for (var i = 0; i < values.length; i++) {
      var pt = elC(doc, "pt");
      nAttr(pt, "idx", String(i));
      pt.appendChild(addText(elC(doc, "v"), isNum ? numStr(values[i]) : String(values[i])));
      cache.appendChild(pt);
    }
    return cache;
  }
  function chartCatAxD(doc) {
    var ax = elC(doc, "catAx");
    ax.appendChild(cVal(doc, "axId", CHART_AXIS_CAT));
    var sc = elC(doc, "scaling");
    sc.appendChild(cVal(doc, "orientation", "minMax"));
    ax.appendChild(sc);
    ax.appendChild(cVal(doc, "delete", 0));
    ax.appendChild(cVal(doc, "axPos", "b"));
    ax.appendChild(cVal(doc, "majorTickMark", "out"));
    ax.appendChild(cVal(doc, "minorTickMark", "none"));
    ax.appendChild(cVal(doc, "tickLblPos", "nextTo"));
    ax.appendChild(cVal(doc, "crossAx", CHART_AXIS_VAL));
    ax.appendChild(cVal(doc, "crosses", "autoZero"));
    ax.appendChild(cVal(doc, "auto", 1));
    ax.appendChild(cVal(doc, "lblAlgn", "ctr"));
    ax.appendChild(cVal(doc, "lblOffset", 100));
    ax.appendChild(cVal(doc, "noMultiLvlLbl", 0));
    return ax;
  }
  function chartValAxD(doc) {
    var ax = elC(doc, "valAx");
    ax.appendChild(cVal(doc, "axId", CHART_AXIS_VAL));
    ax.appendChild(elC(doc, "scaling"));
    ax.appendChild(cVal(doc, "delete", 0));
    ax.appendChild(cVal(doc, "axPos", "l"));
    ax.appendChild(elC(doc, "majorGridlines"));
    ax.appendChild(cVal(doc, "majorTickMark", "out"));
    ax.appendChild(cVal(doc, "minorTickMark", "none"));
    ax.appendChild(cVal(doc, "tickLblPos", "nextTo"));
    ax.appendChild(cVal(doc, "crossAx", CHART_AXIS_CAT));
    ax.appendChild(cVal(doc, "crosses", "autoZero"));
    return ax;
  }
  /* chartSpace(DOM 构造,**不拼字符串**;文本走 createTextNode —— §12.6 第 12 条) */
  function chartXml(spec) {
    var type = String(spec && spec.type ? spec.type : "");
    if (!has(CHART_TYPES, type)) {
      return fail("EINVAL", "图表类型必须是 bar / line / pie(收到:" + (type || "(空)") + ")");
    }
    if (!spec.series || !spec.series.length) { return fail("EINVAL", "图表至少要有一个系列"); }
    var sk = xmlSkeleton("c:chartSpace", [["xmlns:c", NS.c], ["xmlns:a", NS.a], ["xmlns:r", NS.r]]);
    if (!sk.ok) { return sk; }
    var doc = sk.doc, root = doc.documentElement, i;
    root.appendChild(cVal(doc, "date1904", 0));
    var chart = elC(doc, "chart");
    root.appendChild(chart);
    /* 标题:有 ⇒ c:title + autoTitleDeleted=0;无 ⇒ 只写 autoTitleDeleted=1(§12.6 第 3 条) */
    if (spec.title != null && String(spec.title) !== "") {
      var title = elC(doc, "title"), tx = elC(doc, "tx"), rich = elC(doc, "rich");
      rich.appendChild(elA(doc, "bodyPr"));
      rich.appendChild(elA(doc, "lstStyle"));
      var ap = elA(doc, "p");
      var ar = elA(doc, "r");
      ar.appendChild(addText(elA(doc, "t"), spec.title));
      ap.appendChild(ar);
      rich.appendChild(ap);
      tx.appendChild(rich);
      title.appendChild(tx);
      title.appendChild(elC(doc, "layout"));
      title.appendChild(cVal(doc, "overlay", 0));
      chart.appendChild(title);
      chart.appendChild(cVal(doc, "autoTitleDeleted", 0));
    } else {
      chart.appendChild(cVal(doc, "autoTitleDeleted", 1));
    }
    var plot = elC(doc, "plotArea");
    chart.appendChild(plot);
    var isPie = (type === "pie");
    var group = elC(doc, isPie ? "pieChart" : (type === "bar" ? "barChart" : "lineChart"));
    plot.appendChild(group);
    /* c:varyColors 只在饼上取 1(§12.6 第 10 条);柱 / 折线取 0(§12.6 第 16 条,F 轮补记)——
       该元素缺省为 true ⇒ 单系列柱 / 折线会被 PowerPoint 渲染成"逐点着色 + 图例列分类名"
       (F 轮 COM 差分实证:legendEntries 4→1、2)。位置必须在 grouping 之后、series 之前。 */
    if (isPie) {
      group.appendChild(cVal(doc, "varyColors", 1));
    } else if (type === "bar") {
      group.appendChild(cVal(doc, "barDir", "col"));
      group.appendChild(cVal(doc, "grouping", "clustered"));
      group.appendChild(cVal(doc, "varyColors", 0));
    } else {
      group.appendChild(cVal(doc, "grouping", "standard"));
      group.appendChild(cVal(doc, "varyColors", 0));
    }
    for (i = 0; i < spec.series.length; i++) { group.appendChild(chartSerD(doc, spec.series[i], i, spec)); }
    if (!isPie) {
      group.appendChild(cVal(doc, "axId", CHART_AXIS_CAT));
      group.appendChild(cVal(doc, "axId", CHART_AXIS_VAL));
      plot.appendChild(chartCatAxD(doc));
      plot.appendChild(chartValAxD(doc));
    }
    chart.appendChild(elC(doc, "legend"));
    chart.appendChild(cVal(doc, "dispBlanksAs", "gap"));
    var txPr = elC(doc, "txPr");
    txPr.appendChild(elA(doc, "bodyPr"));
    txPr.appendChild(elA(doc, "lstStyle"));
    var tp = elA(doc, "p"), pPr = elA(doc, "pPr");
    var defRPr = elA(doc, "defRPr");
    nAttr(defRPr, "sz", "1800");
    pPr.appendChild(defRPr);
    tp.appendChild(pPr);
    var epr = elA(doc, "endParaRPr");
    nAttr(epr, "lang", "en-US");
    tp.appendChild(epr);
    txPr.appendChild(tp);
    root.appendChild(txPr);
    /* c:externalData —— §12.6 第 1 条:在 c:chart **之后**;它是"内嵌工作簿"的指针,
       缺了 PowerPoint 报"图表数据不可用"(§12.7 的"为什么必须有") */
    var ed = elC(doc, "externalData");
    rAttr(ed, "id", String(spec.embedRelId || "rId1"));
    ed.appendChild(cVal(doc, "autoUpdate", 0));
    root.appendChild(ed);
    var s = xmlSerialize(doc);
    if (!s.ok) { return s; }
    return { ok: true, text: s.text };
  }

  /* chart 归一化 + 载荷侧交叉判据(§12.5 / §2.2 ⑦ 的载荷侧兜底 —— 工具层
     officeChartErr 已在参数面判过一遍;**这里再判一遍是载荷层自保**,
     因为 OfficeWrite._layers 可被验收脚本直呼、绕过工具层) */
  function normChartOf(ch) {
    if (!ch || typeof ch !== "object" || ch instanceof Array) {
      return fail("EINVAL", "chart 必须是对象({type, categories, series})");
    }
    var type = String(ch.type == null ? "" : ch.type);
    if (!has(CHART_TYPES, type)) {
      return fail("EINVAL", "chart.type 必须是 bar / line / pie(收到:" + (type || "(空)") + ")");
    }
    var cats = ch.categories, ss = ch.series, i, j;
    if (!(cats instanceof Array) || !cats.length) { return fail("EINVAL", "chart.categories 必填且必须是非空数组"); }
    if (cats.length > LIMITS.maxChartPoints) {
      return fail("EINVAL", "chart.categories 最多 " + LIMITS.maxChartPoints + " 项(收到 " + cats.length + " 项)");
    }
    var catList = [];
    for (i = 0; i < cats.length; i++) {
      if (typeof cats[i] !== "string") { return fail("EINVAL", "chart.categories[" + i + "] 必须是字符串"); }
      catList.push(String(cats[i]));
    }
    if (!(ss instanceof Array) || !ss.length) { return fail("EINVAL", "chart.series 至少一个"); }
    if (ss.length > LIMITS.maxChartSeries) {
      return fail("EINVAL", "chart.series 最多 " + LIMITS.maxChartSeries + " 个系列(收到 " + ss.length + " 个)");
    }
    if (type === "pie" && ss.length > 1) {
      return fail("EINVAL", "饼图(pie)只支持 1 个系列(收到 " + ss.length + " 个);多系列请改用 bar 或 line");
    }
    var list = [];
    for (i = 0; i < ss.length; i++) {
      var it = ss[i];
      if (!it || typeof it !== "object" || it instanceof Array) { return fail("EINVAL", "chart.series[" + i + "] 必须是对象"); }
      if (typeof it.name !== "string") { return fail("EINVAL", "chart.series[" + i + "].name 必须是字符串"); }
      var vs = it.values;
      if (!(vs instanceof Array) || !vs.length) { return fail("EINVAL", "chart.series[" + i + "].values 必填且必须是非空数组"); }
      if (vs.length !== catList.length) {
        return fail("EINVAL", "chart.series[" + i + "].values 长度(" + vs.length
          + ")必须等于 categories 长度(" + catList.length + ")");
      }
      var vals = [];
      for (j = 0; j < vs.length; j++) {
        if (typeof vs[j] !== "number" || !isFinite(vs[j])) {
          return fail("EINVAL", "chart.series[" + i + "].values[" + j + "] 必须是有限数值(§12.6 第 13 条)");
        }
        vals.push(vs[j]);
      }
      list.push({ name: String(it.name), values: vals });
    }
    return {
      ok: true,
      chart: {
        type: type, title: (ch.title == null ? null : String(ch.title)),
        categories: catList, series: list
      }
    };
  }
  /* Sheet1!$B$2:$B$5(§12.6 第 8 条;表名固定 Sheet1 ⇒ 无空格 / 中文,不加单引号) */
  function chartRefOf(col, r1, r2) {
    var c = "$" + colName(col) + "$";
    return SHEET_NAME + "!" + c + r1 + (r1 === r2 ? "" : ":" + c + r2);
  }
  /* 网格 + 引用串 —— **同一批坐标**算出来(§12.7 值一致性 / T13 的判据对象):
     A1 空 / B1.. = 系列名;A2.. = 分类名;B2.. = 系列1 值 … */
  function chartGridOf(c) {
    var grid = [], head = [null], i, j;
    for (i = 0; i < c.series.length; i++) { head.push(c.series[i].name); }
    grid.push(head);
    for (j = 0; j < c.categories.length; j++) {
      var row = [c.categories[j]];
      for (i = 0; i < c.series.length; i++) { row.push(c.series[i].values[j]); }
      grid.push(row);
    }
    var last = c.categories.length + 1;
    var refs = { catRef: chartRefOf(0, 2, last), nameRefs: [], valRefs: [] };
    for (i = 0; i < c.series.length; i++) {
      refs.nameRefs.push(chartRefOf(i + 1, 1, 1));
      refs.valRefs.push(chartRefOf(i + 1, 2, last));
    }
    return { grid: grid, refs: refs, lastRow: last };
  }
  /* 图表 / 内嵌工作簿的部件名(§12.2 命名不撞车规则:N = 现有同名序列最大号 + 1,
     **两个序列各自算**(nextChartIndex / nextEmbeddingIndex);我们从零同时建两件 ⇒
     常态下两者同号(与真值同号,便于对拍);包内若有**孤儿工作簿**,工作簿号会跳过它
     而不是覆盖它 —— F-2 落地 P3-1 前此处 n 一值两用) */
  function chartPartOf(fileType, n) {
    return (fileType === "pptx" ? "ppt/charts/chart" : "word/charts/chart") + n + ".xml";
  }
  function embeddingPartOf(fileType, n) {
    /* PowerPoint 用 Microsoft_Excel_SheetN.xlsx、Word 用 Microsoft_Excel_WorksheetN.xlsx
       (§12.7 尾注:PowerPoint 用 Sheet、Word 用 Worksheet)。行为按 **Word 惯例 + 规范
       推断** —— 真值件 gt-chart.docx **未产出**(progress/office-tool-f-done.md §四:
       两条采集路线均卡在 Documents.SaveAs2),U9 仍 `?`,这一条**未对拍**;将来拿到
       真值再按真值定稿(批次 F-2 更正:原注释写"对拍后定稿",与 progress 说法相反) */
    return (fileType === "pptx" ? "ppt/embeddings/Microsoft_Excel_Sheet" : "word/embeddings/Microsoft_Excel_Worksheet")
      + n + ".xlsx";
  }
  function nextChartIndex(entries, fileType) {
    var pre = (fileType === "pptx") ? "ppt/charts/chart" : "word/charts/chart";
    var max = 0, k, m, v;
    for (k in entries) {
      if (!has(entries, k)) { continue; }
      if (k.indexOf(pre) !== 0) { continue; }
      m = new RegExp("^" + pre.replace(/\//g, "\\/") + "([0-9]+)\\.xml$").exec(k);
      if (m) { v = parseInt(m[1], 10); if (v > max) { max = v; } }
    }
    return max + 1;
  }
  /* 内嵌工作簿的独立编号序列(§12.2 的"两个序列各自算";P3-1):
     与图表序列**互不影响** —— 否则包内既有孤儿 Microsoft_Excel_Sheet(N+1).xlsx
     (N = 图表序列 max)时会正好被我们覆盖,违反"不碰就不变" */
  function nextEmbeddingIndex(entries, fileType) {
    var pre = (fileType === "pptx") ? "ppt/embeddings/Microsoft_Excel_Sheet"
      : "word/embeddings/Microsoft_Excel_Worksheet";
    var max = 0, k, m, v;
    for (k in entries) {
      if (!has(entries, k)) { continue; }
      if (k.indexOf(pre) !== 0) { continue; }
      m = new RegExp("^" + pre.replace(/\//g, "\\/") + "([0-9]+)\\.xlsx$").exec(k);
      if (m) { v = parseInt(m[1], 10); if (v > max) { max = v; } }
    }
    return max + 1;
  }
  /* 图表自身的 rels(§12.2②:唯一一条 package 关系 → 内嵌工作簿) */
  function chartRelsText(embedTarget) {
    return simplePart("Relationships", [["xmlns", NS.pr]], function (doc, root) {
      addRel(doc, root, "rId1", REL_PACKAGE, embedTarget);
      return null;
    });
  }
  /* Content_Types 的**一个事务**改写(chart Override + xlsx Default 一次做完,
     免得两次各自序列化互相覆盖) */
  function ctUpdate(entries, mutate) {
    var raw = partText(entries, PART.contentTypes);
    if (raw == null) { return fail("EOFFICE", "缺少部件 " + PART.contentTypes); }
    var par = xmlParse(raw);
    if (!par.ok) { return fail(par.code, PART.contentTypes + " 解析失败:" + par.error); }
    var changed = mutate(par.doc, par.doc.documentElement);
    if (changed && changed.ok === false) { return changed; }
    if (!changed) { return { ok: true, text: null }; }
    var s = xmlSerialize(par.doc, xmlDeclOf(raw));
    if (!s.ok) { return s; }
    return { ok: true, text: s.text };
  }
  function ctHasOverride(doc, partName) {
    var ovs = doc.getElementsByTagNameNS(NS.ct, "Override"), i;
    for (i = 0; i < ovs.length; i++) {
      if (String(ovs[i].getAttribute("PartName") || "") === "/" + partName) { return true; }
    }
    return false;
  }
  function ctAddOverride(doc, root, partName, contentType) {
    var ov = elNs(doc, NS.ct, "Override");
    ov.setAttribute("PartName", "/" + partName);
    ov.setAttribute("ContentType", contentType);
    root.appendChild(ov);
    return true;
  }
  function ctHasDefault(doc, ext) {
    var ds = doc.getElementsByTagNameNS(NS.ct, "Default"), i;
    for (i = 0; i < ds.length; i++) {
      if (String(ds[i].getAttribute("Extension") || "").toLowerCase() === ext) { return true; }
    }
    return false;
  }
  /* Default 插在最后一个 Default 之后(CT_Types 是 (Default|Override)* ⇒ 顺序自由,
     插这里只是为了人读时聚在一起) */
  function ctAddDefault(doc, root, ext, contentType) {
    var ds = doc.getElementsByTagNameNS(NS.ct, "Default");
    var d = elNs(doc, NS.ct, "Default");
    d.setAttribute("Extension", ext);
    d.setAttribute("ContentType", contentType);
    if (ds && ds.length) { root.insertBefore(d, ds[ds.length - 1].nextSibling); }
    else { root.insertBefore(d, root.firstChild); }
    return true;
  }

  /* ---------------- S30:pptx 宿主 + 6 处同步(§12.2 / §12.3) ---------------- */
  /* 形状 id:该幻灯内现有 p:cNvPr/@id 的最大值 + 1(§12.3;必须在整张幻灯内唯一) */
  function nextShapeId(doc) {
    var max = 0, els = doc.getElementsByTagName("*"), i, a, v;
    for (i = 0; i < els.length; i++) {
      if (els[i].namespaceURI !== NS.p || els[i].localName !== "cNvPr") { continue; }
      a = els[i].getAttribute("id");
      if (!a) { continue; }
      v = parseInt(a, 10);
      if (!isNaN(v) && v > max) { max = v; }
    }
    return max + 1;
  }
  /* p:graphicFrame(§12.3 逐字骨架)—— 父元素必须是 a:graphic(不是 p:graphic) */
  function pptxGraphicFrame(doc, id, name, chartRid) {
    var gf = elP(doc, "graphicFrame");
    var nv = elP(doc, "nvGraphicFramePr");
    var cn = elP(doc, "cNvPr"); nAttr(cn, "id", id); nAttr(cn, "name", name);
    nv.appendChild(cn);
    var cg = elP(doc, "cNvGraphicFramePr");
    var locks = elA(doc, "graphicFrameLocks"); nAttr(locks, "noGrp", "1");
    cg.appendChild(locks);
    nv.appendChild(cg);
    nv.appendChild(elP(doc, "nvPr"));
    gf.appendChild(nv);
    var xf = elP(doc, "xfrm");
    var off = elA(doc, "off"); nAttr(off, "x", PPTX_CHART_XF.x); nAttr(off, "y", PPTX_CHART_XF.y);
    var ext = elA(doc, "ext"); nAttr(ext, "cx", PPTX_CHART_XF.cx); nAttr(ext, "cy", PPTX_CHART_XF.cy);
    xf.appendChild(off); xf.appendChild(ext);
    gf.appendChild(xf);
    var g = elA(doc, "graphic");
    var gd = elA(doc, "graphicData");
    nAttr(gd, "uri", URI_CHART);
    var ch = elNs(doc, NS.c, "c:chart");
    rAttr(ch, "id", chartRid);
    gd.appendChild(ch);
    g.appendChild(gd);
    gf.appendChild(g);
    return gf;
  }
  function pptxAddChart(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var norm = normChartOf(spec.chart);
    if (!norm.ok) { return norm; }
    var c = norm.chart;
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var list = pptxSlideList(pkg.entries);
    if (!list.ok) { return list; }
    if (!list.slides.length) { return fail("EOFFICE", "包内没有幻灯,add_chart 无从落点"); }
    var si = (spec.slide_index == null) ? (list.slides.length - 1) : spec.slide_index;
    if (typeof si !== "number" || isNaN(si) || si < 0 || Math.floor(si) !== si) {
      return fail("EINVAL", "slide_index 必须是不小于 0 的整数(省略 = 最后一页)");
    }
    if (si >= list.slides.length) {
      return fail("EINVAL", "slide_index " + si + " 越界(共 " + list.slides.length + " 张)");
    }
    progress(opts, "build");
    var ent = list.slides[si];
    var n = nextChartIndex(pkg.entries, "pptx"), nb = nextEmbeddingIndex(pkg.entries, "pptx");
    var cP = chartPartOf("pptx", n), cR = relsPathOf(cP), wbP = embeddingPartOf("pptx", nb);
    var g = chartGridOf(c);
    var book = xlsxMinimal(g.grid);
    if (!book.ok) { return book; }
    var cx = chartXml({
      type: c.type, title: c.title, categories: c.categories, series: c.series,
      catRef: g.refs.catRef, nameRefs: g.refs.nameRefs, valRefs: g.refs.valRefs, embedRelId: "rId1"
    });
    if (!cx.ok) { return cx; }
    var cr = chartRelsText(relTargetFrom("ppt/charts", wbP));
    if (!cr.ok) { return cr; }
    var rm = relsMapOf(pkg.entries, ent.relsPart);
    if (!rm.ok) { return rm; }
    var chartRid = "rId" + (maxRidOf(rm.map) + 1);
    var writes = {};
    writes[cP] = cx.text;
    writes[cR] = cr.text;
    writes[wbP] = book.bytes;
    var steps = [];
    /* ⑤ slide rels(既有 rel 顺序 / Id **一律不动**) */
    var rw = mutatePart(pkg.entries, ent.relsPart, function (doc) {
      return relAppend(doc, chartRid, REL_CHART, relTargetFrom("ppt/slides", cP));
    });
    if (!rw.ok) { return rw; }
    writes[ent.relsPart] = rw.text;
    /* ⑥ slide:p:spTree 末尾插 p:graphicFrame(§12.3 默认几何 = 16:9 版心) */
    var hostId = 0;
    var sw = mutatePart(pkg.entries, ent.part, function (doc) {
      var tree = pptxSpTree(doc);
      if (!tree) { return fail("EOFFICE", ent.part + " 缺少 p:spTree"); }
      hostId = nextShapeId(doc);
      tree.appendChild(pptxGraphicFrame(doc, hostId, "Chart " + hostId, chartRid));
      return { ok: true, value: { id: hostId } };
    });
    if (!sw.ok) { return sw; }
    writes[ent.part] = sw.text;
    /* ④ Content_Types:chart Override + xlsx Default */
    var ctw = ctUpdate(pkg.entries, function (doc, root) {
      var changed = false;
      if (!ctHasOverride(doc, cP)) { ctAddOverride(doc, root, cP, CT_CHART); changed = true; }
      if (!ctHasDefault(doc, "xlsx")) { ctAddDefault(doc, root, "xlsx", MIME.xlsx); changed = true; }
      return changed;
    });
    if (!ctw.ok) { return ctw; }
    if (ctw.text != null) { writes[PART.contentTypes] = ctw.text; }
    steps.push("第 " + si + " 张追加 p:graphicFrame(cNvPr id=" + hostId + ")并新增关系 " + chartRid
      + " → " + cP);
    steps.push("新增 " + cP + " + " + cR + " + " + wbP + "(" + book.size + " B);"
      + "Content_Types 补 chart Override + xlsx Default");
    steps.push("图表缓存与内嵌工作簿同一批数:" + c.categories.length + " 点 x " + c.series.length + " 系列");
    var fin = finishDraft(pkg, writes, "pptx", false, opts, [cP, cR, wbP],
      [{ part: ent.part, uri: URI_CHART, rid: chartRid }]);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "pptx",
      { charts: 1, series: c.series.length, points: c.categories.length },
      [], steps);
  }

  /* ---------------- S31:docx 宿主(条件项;§12.3 / §14.2 的图片同骨架) ---------------- */
  function docxChartInline(doc, id, name, chartRid) {
    var drawing = elW(doc, "drawing");
    var inline = elNs(doc, NS.wp, "wp:inline");
    inline.setAttribute("distT", "0"); inline.setAttribute("distB", "0");
    inline.setAttribute("distL", "0"); inline.setAttribute("distR", "0");
    var ext = elNs(doc, NS.wp, "wp:extent");
    ext.setAttribute("cx", String(DOCX_CHART_CX)); ext.setAttribute("cy", String(DOCX_CHART_CY));
    inline.appendChild(ext);
    var eff = elNs(doc, NS.wp, "wp:effectExtent");
    eff.setAttribute("l", "0"); eff.setAttribute("t", "0");
    eff.setAttribute("r", "0"); eff.setAttribute("b", "0");
    inline.appendChild(eff);
    var docPr = elNs(doc, NS.wp, "wp:docPr");
    docPr.setAttribute("id", String(id)); docPr.setAttribute("name", name);
    inline.appendChild(docPr);
    var g = elA(doc, "graphic");
    var gd = elA(doc, "graphicData");
    nAttr(gd, "uri", URI_CHART);
    var ch = elNs(doc, NS.c, "c:chart");
    rAttr(ch, "id", chartRid);
    gd.appendChild(ch);
    g.appendChild(gd);
    inline.appendChild(g);
    drawing.appendChild(inline);
    return drawing;
  }
  function docxAddChart(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var norm = normChartOf(spec.chart);
    if (!norm.ok) { return norm; }
    var c = norm.chart;
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var d = docxPartOf(pkg.entries, PART.document);
    if (!d.ok) { return d; }
    var items = docxItems(d.doc), i;
    if (spec.paragraph_index != null) {
      var pi = spec.paragraph_index;
      if (typeof pi !== "number" || isNaN(pi) || pi < 0 || Math.floor(pi) !== pi) {
        return fail("EINVAL", "paragraph_index 必须是不小于 0 的整数(省略 = 正文末尾)");
      }
      if (pi >= items.length) {
        return fail("EINVAL", "paragraph_index " + pi + " 越界(共 " + items.length + " 项)");
      }
    }
    progress(opts, "build");
    var n = nextChartIndex(pkg.entries, "docx"), nb = nextEmbeddingIndex(pkg.entries, "docx");
    var cP = chartPartOf("docx", n), cR = relsPathOf(cP), wbP = embeddingPartOf("docx", nb);
    var g = chartGridOf(c);
    var book = xlsxMinimal(g.grid);
    if (!book.ok) { return book; }
    var cx = chartXml({
      type: c.type, title: c.title, categories: c.categories, series: c.series,
      catRef: g.refs.catRef, nameRefs: g.refs.nameRefs, valRefs: g.refs.valRefs, embedRelId: "rId1"
    });
    if (!cx.ok) { return cx; }
    var cr = chartRelsText(relTargetFrom("word/charts", wbP));
    if (!cr.ok) { return cr; }
    /* word/_rels/document.xml.rels 按需新建(§2.4 的按需 1 件) */
    var hasRels = has(pkg.entries, PART.documentRels);
    var rm = { ok: true, map: {} };
    if (hasRels) {
      rm = relsMapOf(pkg.entries, PART.documentRels);
      if (!rm.ok) { return rm; }
    }
    var chartRid = "rId" + (maxRidOf(rm.map) + 1);
    var writes = {};
    writes[cP] = cx.text;
    writes[cR] = cr.text;
    writes[wbP] = book.bytes;
    var docPrId = 1;
    var pw = mutatePart(pkg.entries, PART.document, function (doc) {
      var body = docxBody(doc);
      if (!body) { return fail("EOFFICE", PART.document + " 缺少 w:body"); }
      var ids = doc.getElementsByTagNameNS(NS.wp, "docPr"), k, v;
      var max = 0;
      for (k = 0; k < ids.length; k++) {
        v = parseInt(String(ids[k].getAttribute("id") || "0"), 10);
        if (!isNaN(v) && v > max) { max = v; }
      }
      docPrId = max + 1;
      /* 锚点必须在**本次 mutate 的这棵树**上现取:mutatePart 会重新解析该部件,
         拿别处解析出来的节点当锚 insertBefore 会抛 DOM 异常(批 F 遗留,add_table /
         add_image 一直是现取的写法) */
      var at = null;
      if (pi != null) {
        var its = docxItems(doc);
        if (pi >= its.length) {
          return fail("EINVAL", "paragraph_index " + pi + " 越界(共 " + its.length + " 项)");
        }
        at = its[pi].nextSibling;
      }
      var p = elW(doc, "p"), r = elW(doc, "r");
      r.appendChild(docxChartInline(doc, docPrId, "Chart " + docPrId, chartRid));
      p.appendChild(r);
      var sectPr = docxBodySectPr(body);
      if (at) { body.insertBefore(p, at); }
      else if (sectPr) { body.insertBefore(p, sectPr); }
      else { body.appendChild(p); }
      return { ok: true, value: { docPrId: docPrId } };
    });
    if (!pw.ok) { return pw; }
    writes[PART.document] = pw.text;
    var steps = [];
    if (hasRels) {
      var rw = mutatePart(pkg.entries, PART.documentRels, function (doc) {
        return relAppend(doc, chartRid, REL_CHART, relTargetFrom("word", cP));
      });
      if (!rw.ok) { return rw; }
      writes[PART.documentRels] = rw.text;
    } else {
      /* 关系链目标由 relsDirOf("word/_rels/document.xml.rels") = "word" 解析 ⇒ charts/chartN.xml */
      var nr = simplePart("Relationships", [["xmlns", NS.pr]], function (doc, root) {
        addRel(doc, root, chartRid, REL_CHART, relTargetFrom("word", cP));
        return null;
      });
      if (!nr.ok) { return nr; }
      writes[PART.documentRels] = nr.text;
      steps.push("新建 " + PART.documentRels + "(按需件)");
    }
    var ctw = ctUpdate(pkg.entries, function (doc, root) {
      var changed = false;
      if (!ctHasOverride(doc, cP)) { ctAddOverride(doc, root, cP, CT_CHART); changed = true; }
      if (!ctHasDefault(doc, "xlsx")) { ctAddDefault(doc, root, "xlsx", MIME.xlsx); changed = true; }
      return changed;
    });
    if (!ctw.ok) { return ctw; }
    if (ctw.text != null) { writes[PART.contentTypes] = ctw.text; }
    steps.push("正文" + (spec.paragraph_index == null ? "末尾" : ("第 " + spec.paragraph_index + " 项之后"))
      + "插入 w:p/w:r/w:drawing/wp:inline(wp:docPr id=" + docPrId + ")并新增关系 " + chartRid + " → " + cP);
    steps.push("新增 " + cP + " + " + cR + " + " + wbP + "(" + book.size + " B);"
      + "Content_Types 补 chart Override + xlsx Default");
    var fin = finishDraft(pkg, writes, "docx", false, opts, [cP, cR, wbP],
      [{ part: PART.document, uri: URI_CHART, rid: chartRid }]);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "docx",
      { charts: 1, series: c.series.length, points: c.categories.length },
      [], steps);
  }

  /* ============================================================
     批次 H:表格(§14.1 / S37)—— docx w:tbl / pptx p:graphicFrame + a:tbl
     ------------------------------------------------------------
     纪律(§14.1):
       · docx **不引用 styles.xml 里的表样式**:边框直接写在 w:tblPr / w:tblBorders,
         warnings 里如实写「表格不继承文档样式」
       · pptx 的 a:tableStyleId 必须 = 模板 ppt/tableStyles.xml 的 def GUID
         (✓ 真值 demo 的 slide19.xml 逐字一致:{5C22544A-7EE6-4342-B048-85BDC9FD1C3A})
       · 插位:docx = paragraph_index 之后(省略 = 正文末尾 w:sectPr 之前);
         pptx = slide_index(省略 = 最后一页)的 p:spTree 末尾
       · **新增 a:graphicData 必须传 opts.addedGraphics**(§12.8⑥):pptx 用本次分配的
         p:cNvPr/@id 当锚(hostId,替代"同部件 + 同 uri"这条偏松的旧锚,F-2 的 N3);
         docx 的 w:tbl 不含 a:graphicData ⇒ 无节点可声明(③ 只对"新增 graphic 节点"生效)
     ============================================================ */
  var TABLE_STYLE_ID = '{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}';
  var REL_IMAGE = REL_DOC_RELS + 'image';
  var EMU_PER_PX = 9525;
  var DOCX_TBL_W = 9026;            /* A4 版心宽 = 11906 - 2 x 1440 twips(§14.1) */
  var DOCX_IMG_MAX_CX = 5486400;    /* docx 版心宽 ≈ 6.0 英寸(§14.2) */
  var PPTX_IMG_MAX_CX = 10363200;   /* 16:9 幻灯宽 - 2 x 914400(§14.2) */
  var PPTX_TBL_ROW_H = 370840;      /* a:tr/@h ≈ 0.4 英寸(§14.1;真值 768096 是 4 行高的表) */
  var IMG_KIND = {
    png: { magic: [0x89, 0x50, 0x4e, 0x47], ext: "png", ct: MIME.png },
    jpeg: { magic: [0xff, 0xd8, 0xff], ext: "jpeg", ct: MIME.jpeg }
  };

  /* 表参数归一(与 officeSpecCheck 的第 ⑦ 步同判据:各行列数必须一致) */
  function normTableOf(tb) {
    if (!tb || typeof tb !== "object" || tb instanceof Array) {
      return fail("EINVAL", "table 必须是对象({rows:[[...]], header:true})");
    }
    var rows = tb.rows, i, j, out = [];
    if (!(rows instanceof Array) || !rows.length) { return fail("EINVAL", "table.rows 必填且必须是非空数组"); }
    if (rows.length > LIMITS.maxTableRows) {
      return fail("EINVAL", "table.rows 最多 " + LIMITS.maxTableRows + " 行(收到 " + rows.length + " 行)");
    }
    var cols = -1;
    for (i = 0; i < rows.length; i++) {
      var r = rows[i], cells = [];
      if (!(r instanceof Array) || !r.length) { return fail("EINVAL", "table.rows[" + i + "] 必须是非空数组"); }
      if (r.length > LIMITS.maxTableCols) {
        return fail("EINVAL", "table.rows[" + i + "] 最多 " + LIMITS.maxTableCols + " 列(收到 " + r.length + " 列)");
      }
      if (cols < 0) { cols = r.length; }
      else if (r.length !== cols) {
        return fail("EINVAL", "table.rows 各行列数必须一致:第 " + i + " 行有 " + r.length
          + " 格,第 0 行有 " + cols + " 格");
      }
      for (j = 0; j < r.length; j++) {
        if (typeof r[j] !== "string") { return fail("EINVAL", "table.rows[" + i + "][" + j + "] 必须是字符串"); }
        cells.push(String(r[j]));
      }
      out.push(cells);
    }
    return { ok: true, table: { rows: out, cols: cols, header: (tb.header == null) ? true : !!tb.header } };
  }
  /* 列宽(twips):按版心均分,末列吃掉除法余数 ⇒ 各列之和**恰好** = DOCX_TBL_W */
  function wTblWidths(cols) {
    var each = Math.floor(DOCX_TBL_W / cols), out = [], i;
    for (i = 0; i < cols; i++) {
      out.push(i === cols - 1 ? (DOCX_TBL_W - each * (cols - 1)) : each);
    }
    return out;
  }
  /* tblPr 的子顺序按 CT_TblPrBase:tblW → tblBorders → tblLayout(顺序错 Word 会提示修复) */
  function wTblBorderOf(doc, name) {
    var e = elW(doc, name);
    wAttr(e, "val", "single"); wAttr(e, "sz", "4"); wAttr(e, "space", "0"); wAttr(e, "color", "auto");
    return e;
  }
  function wTblPrOf(doc) {
    var pr = elW(doc, "tblPr");
    var tw = elW(doc, "tblW"); wAttr(tw, "w", "5000"); wAttr(tw, "type", "pct");
    pr.appendChild(tw);
    var bs = elW(doc, "tblBorders"), dirs = ["top", "left", "bottom", "right", "insideH", "insideV"], i;
    for (i = 0; i < dirs.length; i++) { bs.appendChild(wTblBorderOf(doc, dirs[i])); }
    pr.appendChild(bs);
    var lay = elW(doc, "tblLayout"); wAttr(lay, "type", "fixed");
    pr.appendChild(lay);
    return pr;
  }
  function wTblGridOf(doc, widths) {
    var g = elW(doc, "tblGrid"), i, c;
    for (i = 0; i < widths.length; i++) {
      c = elW(doc, "gridCol"); wAttr(c, "w", String(widths[i]));
      g.appendChild(c);
    }
    return g;
  }
  function wTcOf(doc, text, bold, width) {
    var tc = elW(doc, "tc");
    var pr = elW(doc, "tcPr"), tw = elW(doc, "tcW");
    wAttr(tw, "w", String(width)); wAttr(tw, "type", "dxa");
    pr.appendChild(tw);
    tc.appendChild(pr);
    var p = elW(doc, "p");
    p.appendChild(wRun(doc, text, bold, false));
    tc.appendChild(p);
    return tc;
  }
  /* w:tbl = tblPr, tblGrid, tr*(CT_Tbl 的顺序即正确性) */
  function wTblOf(doc, table) {
    var tbl = elW(doc, "tbl"), widths = wTblWidths(table.cols), i, j, tr;
    tbl.appendChild(wTblPrOf(doc));
    tbl.appendChild(wTblGridOf(doc, widths));
    for (i = 0; i < table.rows.length; i++) {
      tr = elW(doc, "tr");
      for (j = 0; j < table.rows[i].length; j++) {
        tr.appendChild(wTcOf(doc, table.rows[i][j], (table.header && i === 0), widths[j]));
      }
      tbl.appendChild(tr);
    }
    return tbl;
  }
  function docxAddTable(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var nt = normTableOf(spec.table);
    if (!nt.ok) { return nt; }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var d = docxPartOf(pkg.entries, PART.document);
    if (!d.ok) { return d; }
    var pi = null;
    if (spec.paragraph_index != null) {
      pi = spec.paragraph_index;
      if (typeof pi !== "number" || isNaN(pi) || pi < 0 || Math.floor(pi) !== pi) {
        return fail("EINVAL", "paragraph_index 必须是不小于 0 的整数(省略 = 正文末尾)");
      }
      if (pi >= docxItems(d.doc).length) {
        return fail("EINVAL", "paragraph_index " + pi + " 越界(共 " + docxItems(d.doc).length + " 项)");
      }
    }
    progress(opts, "build");
    var tb = nt.table;
    var pw = mutatePart(pkg.entries, PART.document, function (doc) {
      var body = docxBody(doc);
      if (!body) { return fail("EOFFICE", PART.document + " 缺少 w:body"); }
      var at = null;
      if (pi != null) {
        var its = docxItems(doc);
        if (pi >= its.length) {
          return fail("EINVAL", "paragraph_index " + pi + " 越界(共 " + its.length + " 项)");
        }
        at = its[pi].nextSibling;
      }
      var tbl = wTblOf(doc, tb);
      var sectPr = docxBodySectPr(body);
      if (at) { body.insertBefore(tbl, at); }
      else if (sectPr) { body.insertBefore(tbl, sectPr); }
      else { body.appendChild(tbl); }
      return { ok: true, value: { rows: tb.rows.length, cols: tb.cols } };
    });
    if (!pw.ok) { return pw; }
    var writes = {};
    writes[PART.document] = pw.text;
    var steps = ["正文" + (pi == null ? "末尾" : ("第 " + pi + " 项之后")) + "插入 w:tbl("
      + tb.rows.length + " 行 x " + tb.cols + " 列;边框直接格式,列宽按版心 " + DOCX_TBL_W + " twips 均分)"];
    var warns = ["表格边框为直接格式,不继承文档样式(w:tblPr/w:tblBorders 自带边框,未引用 styles.xml 的表样式)"];
    var fin = finishDraft(pkg, writes, "docx", false, opts, [], []);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "docx", { tables: 1 }, warns, steps);
  }

  /* a:tbl(§14.1;真值 slide19.xml 逐字对拍:a:tblPr firstRow/bandRow → a:tableStyleId
     → a:tblGrid/a:gridCol → a:tr/@h → a:tc/(a:txBody + a:tcPr)) */
  function aTblOf(doc, table, widths) {
    var tbl = elA(doc, "tbl"), i, j, tr, tc, tb, tcPr, p, r, rPr;
    var pr = elA(doc, "tblPr");
    nAttr(pr, "firstRow", "1"); nAttr(pr, "bandRow", "1");
    var sid = elA(doc, "tableStyleId");
    addText(sid, TABLE_STYLE_ID);
    pr.appendChild(sid);
    tbl.appendChild(pr);
    var grid = elA(doc, "tblGrid"), gc;
    for (i = 0; i < widths.length; i++) {
      gc = elA(doc, "gridCol"); nAttr(gc, "w", String(widths[i]));
      grid.appendChild(gc);
    }
    tbl.appendChild(grid);
    for (i = 0; i < table.rows.length; i++) {
      tr = elA(doc, "tr"); nAttr(tr, "h", String(PPTX_TBL_ROW_H));
      for (j = 0; j < table.rows[i].length; j++) {
        tc = elA(doc, "tc");
        tb = elA(doc, "txBody");
        tb.appendChild(elA(doc, "bodyPr"));
        tb.appendChild(elA(doc, "lstStyle"));
        p = elA(doc, "p");
        r = elA(doc, "r");
        rPr = elA(doc, "rPr");
        nAttr(rPr, "lang", "zh-CN"); nAttr(rPr, "altLang", "en-US");
        if (table.header && i === 0) { nAttr(rPr, "b", "1"); }
        r.appendChild(rPr);
        r.appendChild(aText(doc, table.rows[i][j]));
        p.appendChild(r);
        tb.appendChild(p);
        tc.appendChild(tb);
        tc.appendChild(elA(doc, "tcPr"));
        tr.appendChild(tc);
      }
      tbl.appendChild(tr);
    }
    return tbl;
  }
  /* p:graphicFrame 宿主(§12.3 / §14.1):几何 = 16:9 版心,高度按行数 */
  function pptxTableGraphicFrame(doc, id, name, table) {
    var gf = elP(doc, "graphicFrame");
    var nv = elP(doc, "nvGraphicFramePr");
    var cn = elP(doc, "cNvPr"); nAttr(cn, "id", id); nAttr(cn, "name", name);
    nv.appendChild(cn);
    var cg = elP(doc, "cNvGraphicFramePr");
    var locks = elA(doc, "graphicFrameLocks"); nAttr(locks, "noGrp", "1");
    cg.appendChild(locks);
    nv.appendChild(cg);
    nv.appendChild(elP(doc, "nvPr"));
    gf.appendChild(nv);
    var cx = PPTX_CHART_XF.cx, each = Math.floor(cx / table.cols), widths = [], i;
    for (i = 0; i < table.cols; i++) {
      widths.push(i === table.cols - 1 ? (cx - each * (table.cols - 1)) : each);
    }
    var xf = elP(doc, "xfrm");
    var off = elA(doc, "off"); nAttr(off, "x", PPTX_CHART_XF.x); nAttr(off, "y", PPTX_CHART_XF.y);
    var ext = elA(doc, "ext"); nAttr(ext, "cx", String(cx));
    nAttr(ext, "cy", String(table.rows.length * PPTX_TBL_ROW_H));
    xf.appendChild(off); xf.appendChild(ext);
    gf.appendChild(xf);
    var g = elA(doc, "graphic");
    var gd = elA(doc, "graphicData");
    nAttr(gd, "uri", URI_TABLE);
    gd.appendChild(aTblOf(doc, table, widths));
    g.appendChild(gd);
    gf.appendChild(g);
    return gf;
  }
  function pptxAddTable(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var nt = normTableOf(spec.table);
    if (!nt.ok) { return nt; }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var list = pptxSlideList(pkg.entries);
    if (!list.ok) { return list; }
    if (!list.slides.length) { return fail("EOFFICE", "包内没有幻灯,add_table 无从落点"); }
    var si = (spec.slide_index == null) ? (list.slides.length - 1) : spec.slide_index;
    if (typeof si !== "number" || isNaN(si) || si < 0 || Math.floor(si) !== si) {
      return fail("EINVAL", "slide_index 必须是不小于 0 的整数(省略 = 最后一页)");
    }
    if (si >= list.slides.length) {
      return fail("EINVAL", "slide_index " + si + " 越界(共 " + list.slides.length + " 张)");
    }
    progress(opts, "build");
    var ent = list.slides[si], tb = nt.table, hostId = 0;
    var sw = mutatePart(pkg.entries, ent.part, function (doc) {
      var tree = pptxSpTree(doc);
      if (!tree) { return fail("EOFFICE", ent.part + " 缺少 p:spTree"); }
      hostId = nextShapeId(doc);
      tree.appendChild(pptxTableGraphicFrame(doc, hostId, "Table " + hostId, tb));
      return { ok: true, value: { id: hostId } };
    });
    if (!sw.ok) { return sw; }
    var writes = {};
    writes[ent.part] = sw.text;
    var steps = ["第 " + si + " 张追加 p:graphicFrame(cNvPr id=" + hostId + ") + a:tbl("
      + tb.rows.length + " 行 x " + tb.cols + " 列);a:tableStyleId = 模板 tableStyles.xml 的 def"];
    /* 表格没有关系引用 ⇒ 用本次分配的 p:cNvPr/@id 当锚(§12.8⑥ 的 hostId 形态) */
    var fin = finishDraft(pkg, writes, "pptx", false, opts, [],
      [{ part: ent.part, uri: URI_TABLE, hostId: hostId }]);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "pptx", { tables: 1 }, [], steps);
  }

  /* ============================================================
     批次 H:图片(§14.2 / S38)—— docx w:drawing/wp:inline / pptx p:pic
     ------------------------------------------------------------
     · 媒体部件:word/media/imageN.<ext> / ppt/media/imageN.<ext>
       (N = 现有同前缀最大号 + 1);Content_Types 补 **Default**(png / jpeg,不给 Override)
     · MIME 白名单 = png / jpeg;**扩展名 + 魔数双判**(png 89 50 4E 47、jpeg FF D8 FF):
       魔数不在白名单、或与扩展名不在同一族 ⇒ ENOTSUP;单张 > maxImageBytes ⇒ E2BIG;
       单次张数 > maxImagesPerCall、或本次总量 > maxImagesPerCall x maxImageBytes ⇒ E2BIG
     · 尺寸:px → EMU(1 px = 9525);只给一边 ⇒ 等比;都不给 ⇒ 原像素尺寸;
       超版心(docx 5486400 / pptx 10363200 EMU)⇒ 等比缩到版心宽,这次缩放写进 warnings
     · **新增 a:graphicData 必须传 opts.addedGraphics**(§12.8⑥):两套宿主都带本次分配的
       关系 id(图片走 r:embed ⇒ rid),pptx 另带 hostId(p:cNvPr/@id)作严锚
     ============================================================ */
  function be32(u8, i) {
    return (u8[i] * 16777216) + (u8[i + 1] * 65536) + (u8[i + 2] * 256) + u8[i + 3];
  }
  /* JPEG 尺寸:扫段找 SOFn(0xC0..0xCF 里除 C4/C8/CC),高在 +5、宽在 +7(SOS 之后不再扫) */
  function jpegSizeOf(u8) {
    var i = 2, n = u8.length, m, len;
    while (i + 8 < n) {
      if (u8[i] !== 0xff) { i++; continue; }
      m = u8[i + 1];
      if (m === 0xff) { i++; continue; }
      if (m === 0x01 || (m >= 0xd0 && m <= 0xd9)) { i += 2; continue; }
      len = (u8[i + 2] << 8) + u8[i + 3];
      if (len < 2) { return null; }
      if (m >= 0xc0 && m <= 0xcf && m !== 0xc4 && m !== 0xc8 && m !== 0xcc) {
        return { w: (u8[i + 7] << 8) + u8[i + 8], h: (u8[i + 5] << 8) + u8[i + 6] };
      }
      if (m === 0xda) { return null; }
      i += 2 + len;
    }
    return null;
  }
  /* 像素尺寸:PNG 只认 IHDR 那处(签名 8 + 长度 4 + 类型串 4 ⇒ 宽在 16、高在 20,大端) */
  function imgSizeOf(u8, kind) {
    if (kind === "png") {
      if (u8.length < 24) { return null; }
      var w = be32(u8, 16), h = be32(u8, 20);
      return (w && h) ? { w: w, h: h } : null;
    }
    return jpegSizeOf(u8);
  }
  function imgMagicOf(u8) {
    var k;
    for (k in IMG_KIND) {
      if (!has(IMG_KIND, k)) { continue; }
      if (startsWithBytes(u8, IMG_KIND[k].magic)) { return k; }
    }
    return null;
  }
  function nextMediaIndex(entries, fileType) {
    var pre = (fileType === "pptx") ? "ppt/media/image" : "word/media/image";
    var re = new RegExp("^" + pre.replace(/\//g, "\\/") + "([0-9]+)\\.[A-Za-z0-9]+$");
    var max = 0, k, m, v;
    for (k in entries) {
      if (!has(entries, k)) { continue; }
      m = re.exec(k);
      if (m) { v = parseInt(m[1], 10); if (v > max) { max = v; } }
    }
    return max + 1;
  }
  function mediaPartOf(fileType, n, ext) {
    return (fileType === "pptx" ? "ppt/media/image" : "word/media/image") + n + "." + ext;
  }
  /* 目标像素尺寸 → EMU(§14.2 的尺寸策略);limited = 是否因超版心被等比缩过 */
  function imgExtentOf(px, wPx, hPx, maxCx) {
    var w = px.w, h = px.h;
    if (wPx != null && hPx != null) { w = wPx; h = hPx; }
    else if (wPx != null) { w = wPx; h = Math.max(1, Math.round(px.h * wPx / px.w)); }
    else if (hPx != null) { h = hPx; w = Math.max(1, Math.round(px.w * hPx / px.h)); }
    var raw = w * EMU_PER_PX, limited = raw > maxCx;
    var cx = limited ? maxCx : raw;
    var cy = limited ? Math.max(1, Math.round(h * EMU_PER_PX * (maxCx / raw))) : h * EMU_PER_PX;
    return { cx: cx, cy: cy, w: w, h: h, limited: limited };
  }
  /* 值域(与 officeSpecCheck 第 ⑤ 步同口径):16 ~ 4096 的整数 */
  function imgPxOf(spec, key) {
    var v = spec ? spec[key] : null;
    if (v == null) { return { ok: true, value: null }; }
    if (typeof v !== "number" || isNaN(v) || Math.floor(v) !== v) {
      return fail("EINVAL", key + " 必须是整数(收到:" + JSON.stringify(v) + ")");
    }
    if (v < 16 || v > 4096) { return fail("EINVAL", key + " 超出取值范围(16 ~ 4096,收到 " + v + ")"); }
    return { ok: true, value: v };
  }
  /* 单张图片归一:字节 → 魔数 → 与扩展名同族 → 尺寸 ⇒ {bytes, kind, ext, px}
     (spec.images 是 appE → OfficeWrite 的内部形态:一张图 = 数组 1 个元素) */
  function imgItemOf(raw, i) {
    var where = "第 " + (i + 1) + " 张图片";
    var b = asBytes(raw ? raw.bytes : null);
    if (!b || !b.length) { return fail("EINVAL", where + "缺少字节(image_bytes)"); }
    if (b.length > LIMITS.maxImageBytes) {
      return fail("E2BIG", where + "超过单张上限 " + LIMITS.maxImageBytes + " 字节(本次 " + b.length + " 字节)");
    }
    var kind = imgMagicOf(b);
    var p = String((raw && raw.path) || "");
    var ext = extOf(p);
    if (!kind) {
      return fail("ENOTSUP", where + "不是 png / jpeg(魔数不在白名单);要插别的格式请先用 ExecutePython 转换");
    }
    if (ext === "png" && kind !== "png") {
      return fail("ENOTSUP", where + "扩展名与内容不符(「" + p + "」是 .png,内容却是 " + kind + ")");
    }
    if ((ext === "jpg" || ext === "jpeg") && kind !== "jpeg") {
      return fail("ENOTSUP", where + "扩展名与内容不符(「" + p + "」是 ." + ext + ",内容却是 " + kind + ")");
    }
    var def = IMG_KIND[kind];
    var px = imgSizeOf(b, kind);
    return { ok: true, item: { bytes: b, kind: kind, ext: def.ext, path: p, px: px } };
  }
  function normImagesOf(spec) {
    var raw = (spec && spec.images instanceof Array) ? spec.images
      : ((spec && spec.image_bytes != null) ? [{ bytes: spec.image_bytes, path: spec.image_path }] : null);
    if (!raw || !raw.length) { return fail("EINVAL", "add_image 缺少图片数据(image_bytes / images)"); }
    if (raw.length > LIMITS.maxImagesPerCall) {
      return fail("E2BIG", "单次最多新增 " + LIMITS.maxImagesPerCall + " 张图片(本次 " + raw.length + " 张)");
    }
    var wPx = imgPxOf(spec, "image_width_px"), hPx = imgPxOf(spec, "image_height_px"), i, total = 0, out = [];
    if (!wPx.ok) { return wPx; }
    if (!hPx.ok) { return hPx; }
    for (i = 0; i < raw.length; i++) {
      var it = imgItemOf(raw[i], i);
      if (!it.ok) { return it; }
      total += it.item.bytes.length;
      out.push(it.item);
    }
    if (total > LIMITS.maxImagesPerCall * LIMITS.maxImageBytes) {
      return fail("E2BIG", "本次图片总量超过上限 " + (LIMITS.maxImagesPerCall * LIMITS.maxImageBytes)
        + " 字节(本次 " + total + " 字节)");
    }
    return { ok: true, images: out, wPx: wPx.value, hPx: hPx.value };
  }
  /* docx 宿主(§14.2;✓ 真值 imgonly.docx 的 w:drawing 骨架):
     w:drawing/wp:inline(distT..R=0) → wp:extent / wp:effectExtent / wp:docPr /
     wp:cNvGraphicFramePr / a:graphic(a:graphicData uri=picture) → pic:pic */
  function docxPicInline(doc, id, name, imgRid, cx, cy) {
    var drawing = elW(doc, "drawing");
    var inline = elNs(doc, NS.wp, "wp:inline");
    inline.setAttribute("distT", "0"); inline.setAttribute("distB", "0");
    inline.setAttribute("distL", "0"); inline.setAttribute("distR", "0");
    var extent = elNs(doc, NS.wp, "wp:extent");
    extent.setAttribute("cx", String(cx)); extent.setAttribute("cy", String(cy));
    inline.appendChild(extent);
    var eff = elNs(doc, NS.wp, "wp:effectExtent");
    eff.setAttribute("l", "0"); eff.setAttribute("t", "0"); eff.setAttribute("r", "0"); eff.setAttribute("b", "0");
    inline.appendChild(eff);
    var docPr = elNs(doc, NS.wp, "wp:docPr");
    docPr.setAttribute("id", String(id)); docPr.setAttribute("name", name);
    inline.appendChild(docPr);
    inline.appendChild(elNs(doc, NS.wp, "wp:cNvGraphicFramePr"));
    var g = elA(doc, "graphic");
    var gd = elA(doc, "graphicData");
    nAttr(gd, "uri", URI_PICTURE);
    var pic = elNs(doc, NS.pic, "pic:pic");
    var nv = elNs(doc, NS.pic, "pic:nvPicPr");
    var cn = elNs(doc, NS.pic, "pic:cNvPr");
    cn.setAttribute("id", String(id)); cn.setAttribute("name", name);
    nv.appendChild(cn);
    nv.appendChild(elNs(doc, NS.pic, "pic:cNvPicPr"));
    pic.appendChild(nv);
    var bf = elNs(doc, NS.pic, "pic:blipFill");
    var blip = elA(doc, "blip");
    rAttr(blip, "embed", imgRid);
    bf.appendChild(blip);
    var stretch = elA(doc, "stretch");
    stretch.appendChild(elA(doc, "fillRect"));
    bf.appendChild(stretch);
    pic.appendChild(bf);
    var spPr = elNs(doc, NS.pic, "pic:spPr");
    var xf = elA(doc, "xfrm");
    var off = elA(doc, "off"); nAttr(off, "x", "0"); nAttr(off, "y", "0");
    var ext = elA(doc, "ext"); nAttr(ext, "cx", String(cx)); nAttr(ext, "cy", String(cy));
    xf.appendChild(off); xf.appendChild(ext);
    spPr.appendChild(xf);
    var geo = elA(doc, "prstGeom"); nAttr(geo, "prst", "rect");
    geo.appendChild(elA(doc, "avLst"));
    spPr.appendChild(geo);
    pic.appendChild(spPr);
    gd.appendChild(pic);
    g.appendChild(gd);
    inline.appendChild(g);
    drawing.appendChild(inline);
    return drawing;
  }
  /* 每张图的 extent + 缩放告警(docx / pptx 共用;maxCx 就是版心宽) */
  function imgPlanOf(norm, maxCx) {
    var plans = [], warns = [], i;
    for (i = 0; i < norm.images.length; i++) {
      var img = norm.images[i];
      if (!img.px && norm.wPx == null && norm.hPx == null) {
        return fail("EINVAL", "无法读取图片「" + img.path + "」的像素尺寸(文件可能损坏或不是标准 "
          + img.kind + "),请用 image_width_px 指定宽度");
      }
      var px = img.px || { w: 1, h: 1 };
      var ex = imgExtentOf(px, norm.wPx, norm.hPx, maxCx);
      plans.push(ex);
      if (ex.limited) {
        warns.push("图片「" + img.path + "」超出版心宽度,已等比缩到版心宽(" + ex.w + " x " + ex.h + " px)");
      }
    }
    return { ok: true, plans: plans, warns: warns };
  }
  function docxAddImage(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var ni = normImagesOf(spec);
    if (!ni.ok) { return ni; }
    var plan0 = imgPlanOf(ni, DOCX_IMG_MAX_CX);
    if (!plan0.ok) { return plan0; }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var d = docxPartOf(pkg.entries, PART.document);
    if (!d.ok) { return d; }
    var pi = null;
    if (spec.paragraph_index != null) {
      pi = spec.paragraph_index;
      if (typeof pi !== "number" || isNaN(pi) || pi < 0 || Math.floor(pi) !== pi) {
        return fail("EINVAL", "paragraph_index 必须是不小于 0 的整数(省略 = 正文末尾)");
      }
      if (pi >= docxItems(d.doc).length) {
        return fail("EINVAL", "paragraph_index " + pi + " 越界(共 " + docxItems(d.doc).length + " 项)");
      }
    }
    progress(opts, "build");
    var i, imgs = ni.images, n0 = nextMediaIndex(pkg.entries, "docx");
    var hasRels = has(pkg.entries, PART.documentRels);
    var rm = { ok: true, map: {} };
    if (hasRels) {
      rm = relsMapOf(pkg.entries, PART.documentRels);
      if (!rm.ok) { return rm; }
    }
    var base = maxRidOf(rm.map), writes = {}, media = [], rids = [], targets = [];
    for (i = 0; i < imgs.length; i++) {
      var mp = mediaPartOf("docx", n0 + i, imgs[i].ext);
      media.push(mp);
      rids.push("rId" + (base + 1 + i));
      targets.push(relTargetFrom("word", mp));
      writes[mp] = imgs[i].bytes;
    }
    var pw = mutatePart(pkg.entries, PART.document, function (doc) {
      var body = docxBody(doc);
      if (!body) { return fail("EOFFICE", PART.document + " 缺少 w:body"); }
      var at = null;
      if (pi != null) {
        var its = docxItems(doc);
        if (pi >= its.length) {
          return fail("EINVAL", "paragraph_index " + pi + " 越界(共 " + its.length + " 项)");
        }
        at = its[pi].nextSibling;
      }
      var ids = doc.getElementsByTagNameNS(NS.wp, "docPr"), k, v, max = 0, made = [];
      for (k = 0; k < ids.length; k++) {
        v = parseInt(String(ids[k].getAttribute("id") || "0"), 10);
        if (!isNaN(v) && v > max) { max = v; }
      }
      for (k = 0; k < imgs.length; k++) {
        var id = max + 1 + k;
        var p = elW(doc, "p"), r = elW(doc, "r");
        r.appendChild(docxPicInline(doc, id, "Image " + id, rids[k],
          plan0.plans[k].cx, plan0.plans[k].cy));
        p.appendChild(r);
        var sectPr = docxBodySectPr(body);
        if (at) { body.insertBefore(p, at); }
        else if (sectPr) { body.insertBefore(p, sectPr); }
        else { body.appendChild(p); }
        made.push(id);
      }
      return { ok: true, value: { ids: made } };
    });
    if (!pw.ok) { return pw; }
    writes[PART.document] = pw.text;
    if (hasRels) {
      var rw = mutatePart(pkg.entries, PART.documentRels, function (doc) {
        for (var k = 0; k < rids.length; k++) { relAppend(doc, rids[k], REL_IMAGE, targets[k]); }
        return { ok: true };
      });
      if (!rw.ok) { return rw; }
      writes[PART.documentRels] = rw.text;
    } else {
      var nr = simplePart("Relationships", [["xmlns", NS.pr]], function (doc, root) {
        for (var k = 0; k < rids.length; k++) { addRel(doc, root, rids[k], REL_IMAGE, targets[k]); }
        return null;
      });
      if (!nr.ok) { return nr; }
      writes[PART.documentRels] = nr.text;
    }
    var ctw = ctUpdate(pkg.entries, function (doc, root) {
      var changed = false;
      for (var k = 0; k < imgs.length; k++) {
        if (!ctHasDefault(doc, imgs[k].ext)) {
          ctAddDefault(doc, root, imgs[k].ext, IMG_KIND[imgs[k].kind].ct);
          changed = true;
        }
      }
      return changed;
    });
    if (!ctw.ok) { return ctw; }
    if (ctw.text != null) { writes[PART.contentTypes] = ctw.text; }
    var added = [], steps = [];
    for (i = 0; i < imgs.length; i++) {
      added.push({ part: PART.document, uri: URI_PICTURE, rid: rids[i] });
      steps.push("新增 " + media[i] + "(" + imgs[i].bytes.length + " B," + imgs[i].kind + ")"
        + (imgs[i].px ? " 源 " + imgs[i].px.w + " x " + imgs[i].px.h + " px" : "")
        + " → 版心内 " + plan0.plans[i].cx + " x " + plan0.plans[i].cy + " EMU;关系 " + rids[i]);
    }
    steps.push("正文" + (pi == null ? "末尾" : ("第 " + pi + " 项之后")) + "插入 w:p/w:r/w:drawing/wp:inline;"
      + (hasRels ? "" : "新建 " + PART.documentRels + "(按需件);")
      + "Content_Types 补 " + imgs.length + " 条图片 Default");
    var fin = finishDraft(pkg, writes, "docx", false, opts, media, added);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "docx", { imagesAdded: imgs.length }, plan0.warns, steps);
  }

  /* pptx 宿主(§14.2;✓ 真值 slide15.xml 的 p:pic 骨架) */
  function pptxPicShape(doc, id, name, descr, imgRid, cx, cy) {
    var pic = elP(doc, "pic");
    var nv = elP(doc, "nvPicPr");
    var cn = elP(doc, "cNvPr"); nAttr(cn, "id", id); nAttr(cn, "name", name); nAttr(cn, "descr", descr);
    nv.appendChild(cn);
    var cnp = elP(doc, "cNvPicPr");
    var locks = elA(doc, "picLocks"); nAttr(locks, "noChangeAspect", "1");
    cnp.appendChild(locks);
    nv.appendChild(cnp);
    nv.appendChild(elP(doc, "nvPr"));
    pic.appendChild(nv);
    var bf = elP(doc, "blipFill");
    var blip = elA(doc, "blip"); rAttr(blip, "embed", imgRid);
    bf.appendChild(blip);
    var stretch = elA(doc, "stretch");
    stretch.appendChild(elA(doc, "fillRect"));
    bf.appendChild(stretch);
    pic.appendChild(bf);
    var spPr = elP(doc, "spPr");
    var xf = elA(doc, "xfrm");
    var off = elA(doc, "off"); nAttr(off, "x", PPTX_CHART_XF.x); nAttr(off, "y", PPTX_CHART_XF.y);
    var ext = elA(doc, "ext"); nAttr(ext, "cx", String(cx)); nAttr(ext, "cy", String(cy));
    xf.appendChild(off); xf.appendChild(ext);
    spPr.appendChild(xf);
    var geo = elA(doc, "prstGeom"); nAttr(geo, "prst", "rect");
    geo.appendChild(elA(doc, "avLst"));
    spPr.appendChild(geo);
    pic.appendChild(spPr);
    return pic;
  }
  function pptxAddImage(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var ni = normImagesOf(spec);
    if (!ni.ok) { return ni; }
    var plan0 = imgPlanOf(ni, PPTX_IMG_MAX_CX);
    if (!plan0.ok) { return plan0; }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var list = pptxSlideList(pkg.entries);
    if (!list.ok) { return list; }
    if (!list.slides.length) { return fail("EOFFICE", "包内没有幻灯,add_image 无从落点"); }
    var si = (spec.slide_index == null) ? (list.slides.length - 1) : spec.slide_index;
    if (typeof si !== "number" || isNaN(si) || si < 0 || Math.floor(si) !== si) {
      return fail("EINVAL", "slide_index 必须是不小于 0 的整数(省略 = 最后一页)");
    }
    if (si >= list.slides.length) {
      return fail("EINVAL", "slide_index " + si + " 越界(共 " + list.slides.length + " 张)");
    }
    progress(opts, "build");
    var ent = list.slides[si], imgs = ni.images, i;
    var n0 = nextMediaIndex(pkg.entries, "pptx");
    var rm = relsMapOf(pkg.entries, ent.relsPart);
    if (!rm.ok) { return rm; }
    var base = maxRidOf(rm.map), writes = {}, media = [], rids = [], targets = [];
    for (i = 0; i < imgs.length; i++) {
      var mp = mediaPartOf("pptx", n0 + i, imgs[i].ext);
      media.push(mp);
      rids.push("rId" + (base + 1 + i));
      targets.push(relTargetFrom("ppt/slides", mp));
      writes[mp] = imgs[i].bytes;
    }
    var rw = mutatePart(pkg.entries, ent.relsPart, function (doc) {
      for (var k = 0; k < rids.length; k++) { relAppend(doc, rids[k], REL_IMAGE, targets[k]); }
      return { ok: true };
    });
    if (!rw.ok) { return rw; }
    writes[ent.relsPart] = rw.text;
    var ids = [];
    var sw = mutatePart(pkg.entries, ent.part, function (doc) {
      var tree = pptxSpTree(doc);
      if (!tree) { return fail("EOFFICE", ent.part + " 缺少 p:spTree"); }
      for (var k = 0; k < imgs.length; k++) {
        var id = nextShapeId(doc);
        tree.appendChild(pptxPicShape(doc, id, "Picture " + id, imgs[k].path,
          rids[k], plan0.plans[k].cx, plan0.plans[k].cy));
        ids.push(id);
      }
      return { ok: true, value: { ids: ids } };
    });
    if (!sw.ok) { return sw; }
    writes[ent.part] = sw.text;
    var ctw = ctUpdate(pkg.entries, function (doc, root) {
      var changed = false;
      for (var k = 0; k < imgs.length; k++) {
        if (!ctHasDefault(doc, imgs[k].ext)) {
          ctAddDefault(doc, root, imgs[k].ext, IMG_KIND[imgs[k].kind].ct);
          changed = true;
        }
      }
      return changed;
    });
    if (!ctw.ok) { return ctw; }
    if (ctw.text != null) { writes[PART.contentTypes] = ctw.text; }
    var steps = [];
    for (i = 0; i < imgs.length; i++) {
      steps.push("第 " + si + " 张追加 p:pic(cNvPr id=" + ids[i] + ") + " + media[i]
        + "(" + imgs[i].bytes.length + " B," + imgs[i].kind + ")"
        + (imgs[i].px ? " 源 " + imgs[i].px.w + " x " + imgs[i].px.h + " px" : "")
        + " → 幻灯内 " + plan0.plans[i].cx + " x " + plan0.plans[i].cy + " EMU;关系 " + rids[i]);
    }
    steps.push("Content_Types 补 " + imgs.length + " 条图片 Default");
    /* ★ pptx 的图片**没有 a:graphicData**:真值 slide15.xml 的 p:pic 直接挂在
       p:spTree 下(结构 = p:nvPicPr / p:blipFill / p:spPr),不像图表 / 表格那样走
       p:graphicFrame > a:graphic > a:graphicData ⇒ 本 op **不新增 graphic 节点**,
       故传空 addedGraphics(§12.8⑥ 的"必须传"只针对新增 graphic 节点的 op;
       §12.8⑦ 里"pptx:p:graphicFrame/a:graphic"指的是图表 / 表格这条链)。
       图片的引用闭合由 ②③④ 覆盖:CT Default(png/jpeg)②、媒体部件存在(rel Target)③、
       p:blipFill/a:blip 的 r:embed 有主④ —— 与 rid 锚同强度。 */
    var fin = finishDraft(pkg, writes, "pptx", false, opts, media, []);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "pptx", { imagesAdded: imgs.length }, plan0.warns, steps);
  }

  /* ============================================================
     批次 H:套用版式(pptx;§14.3 / S39)
     ------------------------------------------------------------
     · 寻址:outline 的 layouts[{i, name, placeholders}] —— i = 母版 p:sldLayoutIdLst
       的顺序(与 pptxLayoutsOf 的 out 顺序同一口径)
     · 只改 ppt/slides/_rels/slideN.xml.rels 里 slideLayout 那一条的 **Target**
       (**Id 不动**、既有条目顺序不动)⇒ 无新部件、无 Content_Types 改动
     · 占位符不匹配(目标版式没有同 (type, idx))⇒ 保留该形状原样 + warnings 点名
       「第 i 个形状在新版式里没有对应占位符,位置可能变化」(§14.3 的降级策略)
     · **绝不删除任何形状文本**(§14.3 硬承诺;本函数不碰幻灯的形状节点)
     · 同值重设 ⇒ 被点名的那个 rels 部件逐字节不变(§13.4 / V12 的幂等口径)
     ============================================================ */
  function pptxApplyLayout(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var si = spec.slide_index, li = spec.layout_index;
    if (typeof si !== "number" || isNaN(si) || si < 0 || Math.floor(si) !== si) {
      return fail("EINVAL", "slide_index 必须是不小于 0 的整数(0 起,取自 outline)");
    }
    if (typeof li !== "number" || isNaN(li) || li < 0 || Math.floor(li) !== li) {
      return fail("EINVAL", "layout_index 必须是不小于 0 的整数(0 起,取自 outline 的 layouts[])");
    }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var list = pptxSlideList(pkg.entries);
    if (!list.ok) { return list; }
    if (si >= list.slides.length) {
      return fail("EINVAL", "slide_index " + si + " 越界(共 " + list.slides.length + " 张)");
    }
    var lay = pptxLayoutsOf(pkg.entries);
    if (!lay.ok) { return lay; }
    if (li >= lay.layouts.length) {
      return fail("EINVAL", "layout_index " + li + " 越界(共 " + lay.layouts.length + " 个版式)");
    }
    var ent = list.slides[si], target = lay.layouts[li], k;
    var rm = relsMapOf(pkg.entries, ent.relsPart);
    if (!rm.ok) { return rm; }
    var relId = null, cur = "";
    for (k in rm.map) {
      if (!has(rm.map, k)) { continue; }
      if (rm.map[k].type !== REL_SLIDE_LAYOUT) { continue; }
      relId = k;
      cur = resolvePartName("ppt/slides", rm.map[k].target);
      break;
    }
    if (!relId) {
      return fail("EOFFICE", ent.relsPart + " 里没有 slideLayout 关系,套用版式无从落点");
    }
    progress(opts, "build");
    var newTarget = relTargetFrom("ppt/slides", target.part);
    var rw = mutatePart(pkg.entries, ent.relsPart, function (doc) {
      var rels = doc.getElementsByTagNameNS(NS.pr, "Relationship"), i;
      for (i = 0; i < rels.length; i++) {
        if (String(rels[i].getAttribute("Id") || "") !== relId) { continue; }
        rels[i].setAttribute("Target", newTarget);
        return { ok: true, value: { target: newTarget } };
      }
      return fail("EOFFICE", ent.relsPart + " 找不到关系 " + relId);
    });
    if (!rw.ok) { return rw; }
    /* 占位符不匹配 ⇒ 只报警告(形状原样保留,§14.3) */
    var d = partDocOf(pkg.entries, ent.part), warns = [], steps = [];
    if (!d.ok) { return d; }
    var shapes = pptxShapesOf(d.doc), j, m, ph, ty, idx, hit;
    for (j = 0; j < shapes.length; j++) {
      ph = pptxPhOf(shapes[j]);
      if (!ph) { continue; }
      ty = pptxPhType(ph);
      idx = pptxPhIdx(ph);
      hit = false;
      for (m = 0; m < target.placeholders.length; m++) {
        if (String(target.placeholders[m].type) === ty && Number(target.placeholders[m].idx) === idx) {
          hit = true;
          break;
        }
      }
      if (!hit) {
        warns.push("第 " + j + " 个形状在新版式(「" + target.name + "」,layout_index=" + li
          + ")里没有对应占位符(type=" + (ty || "(默认)") + ", idx=" + idx + "),位置可能变化");
      }
    }
    var writes = {};
    writes[ent.relsPart] = rw.text;
    steps.push("第 " + si + " 张的版式:" + (cur || "(未知)") + " → " + target.part
      + "(「" + target.name + "」)" + (cur === target.part ? ";同值重设" : "")
      + ";关系 " + relId + " 的 Id 与既有条目顺序未动");
    if (!warns.length) { steps.push("幻灯上所有占位符在新版式里都有同 (type, idx) 对应项"); }
    var fin = finishDraft(pkg, writes, "pptx", false, opts, [], []);
    if (!fin.ok) { return fin; }
    return okResult(fin.bytes, "pptx", { layouts: 1 }, warns, steps);
  }

  /* ============================================================
     validatePackage(方案 §12.8 / §3 S32)—— **写盘前**跑;**S32 的七条清单与编号不动**
     七条断言(批次 B 落 ①–④,批次 F 补 ⑤⑥⑦):
       ① 必需部件齐备(create 的固定件 / 本次新增件)
       ② Content_Types 完整(Override 指向的部件必须存在;每个部件都被覆盖;
          .xml 部件必须有自己的 Override —— 通用 "xml" Default 不算覆盖)
       ③ 关系闭合(每条 rels 的 Target 在包内存在;TargetMode="External" 除外)
       ④ 引用有主(每个 r:id / r:embed / r:link 都能在**同目录**的 .rels 里找到)
       ⑤ 图表链闭合(被引用的 chartN.xml 必须三跳全通:slide rels(chart)→ chartN.xml
          → chart rels(package)→ embeddings/*.xlsx);**孤儿图表**(v1 删页不清理,
          §12.9)不判 —— 否则删过页的包会从此不可再编辑
       ⑥ 内嵌工作簿可解(fflate.unzipSync 成功 + 含 xl/workbook.xml 与
          xl/worksheets/sheet1.xml + workbook.xml 可解析)
       ⑦ 宿主要素名 / 元素父子结构 / a:graphicData/@uri 白名单(§12.8 第 ⑦ 条 / P1-2;
          父子结构一条为批 G 的 P1-b 补):
          · **包级**(对他人包零误伤,合法文件永远成立):不得出现 OOXML 里不存在的
            **p:graphic**;a:graphicData 的父元素必须逐字是 a:graphic;字段节点 **a:fld**
            的父元素必须逐字是 a:p(a:fld 与 a:r 同级直挂 —— a:p = (a:pPr?, EG_TextRun*,
            a:endParaRPr?)、EG_TextRun = a:r | a:br | a:fld,而 a:r 只许 (a:rPr?, a:t) ⇒
            把 a:fld 套进 a:r 是非法结构,批 G 的 P0 就是这么漏出去的)
          · **本次新增的那一个 a:graphicData**(opts.addedGraphics,条目 = {part, uri, rid, hostId}):
            @uri 必须 ∈ {chart, table, picture},且该节点确实落在产出里(按"同部件 + 同 uri +
            内含该 r:id"或"位于 p:cNvPr/@id=hostId 的那个形状内"定位;表格没有关系引用 ⇒
            只用 hostId)。**作用域 = 元素级,不按部件、更不按全包**(F-2 收窄,原 P2-1):
            合法包里的 SmartArt(diagram)/ OLE 的 uri 不在白名单内,而 SmartArt 恰恰可能就在
            我们改写的那张幻灯/那篇正文里 —— 按部件收窄会把它判成产出非法、工具拒写;
            按元素收窄后,只有我们自己插进去的那个节点被核(§12.2 的"零新部件族"
            本来就只约束我们自己新增的东西)
     失败一律 {ok:false, code:"EOFFICE"} —— 调用方据此**不写工作区**。
     ============================================================ */
  var REQ_FIXED = {
    docx: [PART.contentTypes, PART.rootRels, PART.core, PART.app, PART.document, PART.styles, PART.numbering],
    pptx: [PART.contentTypes, PART.rootRels, PART.core, PART.app, PART.presentation, PART.presentationRels,
      PART.theme, PART.slideMaster, PART.slideMasterRels, PART.layout1, PART.layout2,
      'ppt/slideLayouts/_rels/slideLayout1.xml.rels', 'ppt/slideLayouts/_rels/slideLayout2.xml.rels',
      PART.presProps, PART.viewProps, PART.tableStyles]
  };
  var R_ATTRS = { id: true, embed: true, link: true };

  function ctOf(entries) {
    var raw = partText(entries, PART.contentTypes);
    if (raw == null) { return fail("EOFFICE", "产出缺少部件 " + PART.contentTypes); }
    var par = xmlParse(raw);
    if (!par.ok) { return fail(par.code, PART.contentTypes + " 解析失败:" + par.error); }
    var root = par.doc.documentElement, i;
    var defaults = [], overrides = [];
    var ds = root.getElementsByTagNameNS(NS.ct, "Default");
    for (i = 0; i < ds.length; i++) {
      defaults.push(String(ds[i].getAttribute("Extension") || "").replace(/^\./, "").toLowerCase());
    }
    var os = root.getElementsByTagNameNS(NS.ct, "Override");
    for (i = 0; i < os.length; i++) {
      overrides.push({
        name: String(os[i].getAttribute("PartName") || "").replace(/^\//, ""),
        type: String(os[i].getAttribute("ContentType") || "")
      });
    }
    return { ok: true, defaults: defaults, overrides: overrides };
  }
  /* 某个 XML 部件里出现的全部关系引用(r:id / r:embed / r:link) */
  function rIdsIn(doc) {
    var els = doc.getElementsByTagName("*"), out = [], i, j;
    for (i = 0; i < els.length; i++) {
      var attrs = els[i].attributes;
      if (!attrs) { continue; }
      for (j = 0; j < attrs.length; j++) {
        var a = attrs[j];
        if (a.namespaceURI === NS.r && R_ATTRS[a.localName] && a.value) { out.push(String(a.value)); }
      }
    }
    return out;
  }
  function validateEntries(entries, fileType, opts) {
    var o = opts || {};
    if (fileType !== "docx" && fileType !== "pptx") {
      return fail("EINVAL", "validatePackage 需要 file_type(docx 或 pptx)");
    }
    var all = o.requireAll ? REQ_FIXED[fileType] : [PART.contentTypes, PART.rootRels, mainPartOf(fileType)];
    var req = all.slice(), extra = o.addedParts || [];
    var names = Object.keys(entries), i, j, k;
    /* ① 必需件齐备 */
    for (i = 0; i < extra.length; i++) { req.push(extra[i]); }
    for (i = 0; i < req.length; i++) {
      if (!has(entries, req[i])) { return fail("EOFFICE", "产出缺少部件 " + req[i]); }
    }
    var ct = ctOf(entries);
    if (!ct.ok) { return ct; }
    /* ② Content_Types 完整 */
    var ovMap = {}, deft = {};
    for (i = 0; i < ct.overrides.length; i++) {
      if (!ct.overrides[i].name || !has(entries, ct.overrides[i].name)) {
        return fail("EOFFICE", "Content_Types 的 Override 指向不存在的部件 "
          + (ct.overrides[i].name || "(空 PartName)"));
      }
      ovMap[ct.overrides[i].name] = true;
    }
    for (i = 0; i < ct.defaults.length; i++) { if (ct.defaults[i]) { deft[ct.defaults[i]] = true; } }
    for (i = 0; i < names.length; i++) {
      var nm = names[i];
      if (isDirEntry(nm) || nm === PART.contentTypes || ovMap[nm]) { continue; }
      var ext = extOf(nm);
      /* .xml 必须有自己的 Override:模板里的通用 Default xml=application/xml 只是兜底,
         拿它当覆盖会让"新增部件忘补 Override"一路走到 Word 报修复 */
      if (ext === "xml" || !deft[ext]) { return fail("EOFFICE", "Content_Types 未覆盖 " + nm); }
    }
    /* ③ 关系闭合 */
    for (i = 0; i < names.length; i++) {
      var rp = names[i];
      if (isDirEntry(rp) || !/\.rels$/.test(rp)) { continue; }
      var rr = xmlParse(partText(entries, rp));
      if (!rr.ok) { return fail("EOFFICE", rp + " 解析失败:" + rr.error); }
      var rl = rr.doc.getElementsByTagNameNS(NS.pr, "Relationship"), dir = relsDirOf(rp);
      for (j = 0; j < rl.length; j++) {
        if (String(rl[j].getAttribute("TargetMode") || "") === "External") { continue; }
        var target = String(rl[j].getAttribute("Target") || "");
        var full = resolvePartName(dir, target);
        if (!has(entries, full)) {
          return fail("EOFFICE", "关系指向不存在的部件 " + (target || "(空 Target)") + "(" + rp + ")");
        }
      }
    }
    /* ④ 引用有主 */
    for (i = 0; i < names.length; i++) {
      var pn = names[i];
      if (isDirEntry(pn) || !/\.xml$/.test(pn)) { continue; }
      var pp = xmlParse(partText(entries, pn));
      if (!pp.ok) { return fail("EOFFICE", pn + " 不是合法 XML:" + pp.error); }
      var hits = rIdsIn(pp.doc);
      if (!hits.length) { continue; }
      var relsPath = relsPathOf(pn);
      if (!has(entries, relsPath)) {
        return fail("EOFFICE", pn + " 引用了 rId(" + hits[0] + ") 但包内没有 " + relsPath);
      }
      var rrp = xmlParse(partText(entries, relsPath));
      if (!rrp.ok) { return fail("EOFFICE", relsPath + " 解析失败:" + rrp.error); }
      var defs = {}, list2 = rrp.doc.getElementsByTagNameNS(NS.pr, "Relationship");
      for (j = 0; j < list2.length; j++) { defs[String(list2[j].getAttribute("Id") || "")] = true; }
      for (j = 0; j < hits.length; j++) {
        if (!defs[hits[j]]) { return fail("EOFFICE", pn + " 引用了不存在的 rId " + hits[j]); }
      }
    }
    /* ⑤ 图表链闭合(§12.8 第 ⑤ 条) */
    var chain = checkChartChain(entries, o.addedParts || []);
    if (!chain.ok) { return chain; }
    /* ⑥ 内嵌工作簿可解(§12.8 第 ⑥ 条) */
    var books = checkEmbeddedBooks(entries);
    if (!books.ok) { return books; }
    /* ⑦ 宿主要素名 / a:graphicData/@uri 白名单(§12.8 第 ⑦ 条) */
    var hosts = checkGraphicHosts(entries, o.addedGraphics);
    if (!hosts.ok) { return hosts; }
    return {
      ok: true,
      checked: ["required-parts", "content-types", "rels-targets", "rid-owners",
        "chart-chain", "embedded-workbook", "graphic-hosts"],
      pending: [],
      charts: chain.checked, books: books.checked, hosts: hosts.checked
    };
  }
  /* ---------------- S32 的 ⑤⑥⑦(§12.8;批次 F 新增) ---------------- */
  /* ⑤ 图表链闭合:对**被 rels 引用的**每张图表逐跳走
       第 1 跳 = 有部件以 .../relationships/chart 指向它(批内新增的图表**必须**被引用)
       第 2 跳 = 它自己的 rels 里有 .../relationships/package
       第 3 跳 = 那条 package 指向 embeddings/ 下的工作簿且部件真实存在
     addedParts 里的图表件强制要求"被引用"(否则 = §12.2 的六处少写一处、PowerPoint 打不开);
     不在 addedParts 又没人引用的图表 = **孤儿**(v1 的 delete 不清理,§12.9)⇒ 不判。 */
  function checkChartChain(entries, addedParts) {
    var names = Object.keys(entries), i, j, k;
    var chartRe = /^(ppt|word)\/charts\/chart[0-9]+\.xml$/;
    var charts = [], added = {};
    for (i = 0; i < addedParts.length; i++) { added[addedParts[i]] = true; }
    for (i = 0; i < names.length; i++) { if (chartRe.test(names[i])) { charts.push(names[i]); } }
    if (!charts.length) { return { ok: true, checked: 0 }; }
    var refd = {};
    for (i = 0; i < names.length; i++) {
      var rp = names[i];
      if (isDirEntry(rp) || !/\.rels$/.test(rp)) { continue; }
      var rr = xmlParse(partText(entries, rp));
      if (!rr.ok) { return fail("EOFFICE", rp + " 解析失败:" + rr.error); }
      var rl = rr.doc.getElementsByTagNameNS(NS.pr, "Relationship"), dir = relsDirOf(rp);
      for (j = 0; j < rl.length; j++) {
        if (String(rl[j].getAttribute("Type") || "") !== REL_CHART) { continue; }
        refd[resolvePartName(dir, String(rl[j].getAttribute("Target") || ""))] = rp;
      }
    }
    var seen = 0;
    for (k = 0; k < charts.length; k++) {
      var cp = charts[k];
      if (!refd[cp] && !added[cp]) { continue; }
      if (!refd[cp]) {
        return fail("EOFFICE", "图表引用链断在第 1 跳:没有部件以 .../relationships/chart 指向 " + cp);
      }
      var crp = relsPathOf(cp);
      if (!has(entries, crp)) { return fail("EOFFICE", "图表引用链断在第 2 跳:缺少 " + crp); }
      var cr = xmlParse(partText(entries, crp));
      if (!cr.ok) { return fail("EOFFICE", crp + " 解析失败:" + cr.error); }
      var cl = cr.doc.getElementsByTagNameNS(NS.pr, "Relationship"), hit = null;
      for (j = 0; j < cl.length; j++) {
        if (String(cl[j].getAttribute("Type") || "") !== REL_PACKAGE) { continue; }
        hit = resolvePartName(relsDirOf(crp), String(cl[j].getAttribute("Target") || ""));
        break;
      }
      if (!hit) {
        return fail("EOFFICE", "图表引用链断在第 2 跳:" + crp
          + " 里没有 .../relationships/package 关系(图表 → 内嵌工作簿)");
      }
      if (hit.indexOf("embeddings/") < 0) {
        return fail("EOFFICE", "图表的内嵌工作簿必须落在 embeddings/ 下(收到 " + hit + ")");
      }
      if (!has(entries, hit)) { return fail("EOFFICE", "图表引用链断在第 3 跳:缺少 " + hit); }
      seen++;
    }
    return { ok: true, checked: seen };
  }
  /* ⑥ 内嵌工作簿可解:自己解包 + 必需两件在 + workbook.xml 可解析 */
  function checkEmbeddedBooks(entries) {
    var names = Object.keys(entries), list = [], i;
    for (i = 0; i < names.length; i++) {
      if (/^(ppt|word)\/embeddings\/[^\/]+\.xlsx$/.test(names[i])) { list.push(names[i]); }
    }
    if (!list.length) { return { ok: true, checked: 0 }; }
    for (i = 0; i < list.length; i++) {
      var un = zipUnzip(entries[list[i]]);
      if (!un.ok) { return fail("EOFFICE", "内嵌工作簿不可读(" + list[i] + "):" + un.error); }
      if (!has(un.name2bytes, "xl/workbook.xml") || !has(un.name2bytes, "xl/worksheets/sheet1.xml")) {
        return fail("EOFFICE", "内嵌工作簿不可读(" + list[i]
          + "):缺 xl/workbook.xml 或 xl/worksheets/sheet1.xml");
      }
      var pb = xmlParse(bytesToText(un.name2bytes["xl/workbook.xml"]));
      if (!pb.ok) {
        return fail("EOFFICE", "内嵌工作簿不可读(" + list[i] + "):xl/workbook.xml 不是合法 XML");
      }
    }
    return { ok: true, checked: list.length };
  }
  /* ⑦ 宿主要素名 / 元素父子结构 / uri 白名单(作用域见 validatePackage 头注:**三条包级**
     不变量(禁 p:graphic / a:graphicData 的父必须是 a:graphic / a:fld 的父必须是 a:p)
     + 对**本次新增的那一个 a:graphicData**做 uri 白名单;addedGraphics 缺省 = 不做 uri 扫描)
       addedGraphics 条目 = { part:部件名, uri:我们写进去的 uri,
                              rid:宿主里那个 r:id/r:embed(图片 r:embed / 图表 r:id),
                              hostId:pptx 宿主形状的 p:cNvPr/@id(**表格的唯一锚** ——
                                     a:tbl 没有关系引用 ⇒ 没有 rid 可用) }
     —— 元素级定位而非部件级:F-2 起 P2-1 修好,宿主部件里**既有**的 SmartArt / OLE 等
     合法 graphicData 一律不看,不会被误杀。
     **批次 H(F-2 的 N3 收口)**:没有 rid 的宿主(表格)改用**本次分配的 p:cNvPr/@id**
     定位(hostId)—— 旧口径"同部件 + 同 uri 即命中"留着只给直呼本层的探针用,产品路径
     (add_table)不再走它;仍是"元素级",不回到按部件收窄。
     返回值 checked 仍只数 addedGraphics 命中数(批 F 的读数口径不动);第三条不变量
     只加 fail 路径,不增返回项 */
  function checkGraphicHosts(entries, addedGraphics) {
    var names = Object.keys(entries), i, j, k, checked = 0;
    var add = addedGraphics || [], byPart = {}, pend, hit;
    for (k = 0; k < add.length; k++) {
      var ga = add[k];
      if (ga.uri !== URI_CHART && ga.uri !== URI_TABLE && ga.uri !== URI_PICTURE) {
        return fail("EOFFICE", "本次新增的 a:graphicData/@uri 不在白名单 " + ga.uri + "(" + ga.part + ")");
      }
      if (!has(entries, ga.part) || !/\.xml$/.test(ga.part) || isDirEntry(ga.part)) {
        return fail("EOFFICE", "本次新增的 a:graphicData 所在部件不在产出里 " + ga.part);
      }
      if (!byPart[ga.part]) { byPart[ga.part] = []; }
      byPart[ga.part].push(ga);
    }
    for (i = 0; i < names.length; i++) {
      var pn = names[i];
      if (isDirEntry(pn) || !/\.xml$/.test(pn)) { continue; }
      var pp = xmlParse(partText(entries, pn));
      if (!pp.ok) { return fail("EOFFICE", pn + " 不是合法 XML:" + pp.error); }
      var all = pp.doc.getElementsByTagName("*");
      var gds = [], gsid = [];
      for (j = 0; j < all.length; j++) {
        var el = all[j];
        /* 包级不变量 1:OOXML 里不存在 p:graphic(真值 34 张 demo slide 计数 = 0) */
        if (el.namespaceURI === NS.p && el.localName === "graphic") {
          return fail("EOFFICE", "宿主要素名不合法 " + pn + " 出现 p:graphic(应为 a:graphic)");
        }
        /* 包级不变量 3(批 G 的 P1-b):字段节点 a:fld 的父元素必须逐字是 a:p ——
           a:p = (a:pPr?, EG_TextRun*, a:endParaRPr?)、EG_TextRun = a:r | a:br | a:fld,
           而 a:r(CT_RegularTextRun)只许 (a:rPr?, a:t) ⇒ 把 a:fld 套进 a:r 是非法结构
           (批 G 的 P0:PowerPoint 判整包损坏 0x80070570)。a:fld 的唯一合法宿主就是
           a:p(幻灯 / 母版 / 版式 / 图表文本 / SmartArt 数据都写 a:p),合法文件永远成立,
           故与不变量 1/2 同属包级 */
        if (el.namespaceURI === NS.a && el.localName === "fld") {
          var pf = el.parentNode;
          if (!pf || pf.nodeType !== 1 || pf.namespaceURI !== NS.a || pf.localName !== "p") {
            return fail("EOFFICE", "宿主要素父子结构不合法(" + pn + "):a:fld 的父元素是 "
              + (pf && pf.nodeType === 1 ? ((pf.prefix ? pf.prefix + ":" : "") + pf.localName) : "(非元素)")
              + "(应为 a:p —— a:fld 与 a:r 同级直挂,a:r 只许 a:rPr / a:t)");
          }
        }
        if (el.namespaceURI !== NS.a || el.localName !== "graphicData") { continue; }
        /* 包级不变量 2:a:graphicData 的父元素必须逐字是 a:graphic */
        var pa = el.parentNode;
        if (!pa || pa.nodeType !== 1 || pa.namespaceURI !== NS.a || pa.localName !== "graphic") {
          return fail("EOFFICE", "宿主要素名不合法(" + pn + "):a:graphicData 的父元素是 "
            + (pa && pa.nodeType === 1 ? ((pa.prefix ? pa.prefix + ":" : "") + pa.localName) : "(非元素)")
            + "(应为 a:graphic)");
        }
        gds.push(el);
        gsid.push(enclosingShapeId(el));
      }
      if (!byPart[pn]) { continue; }
      pend = byPart[pn];
      for (k = 0; k < pend.length; k++) {
        hit = false;
        for (j = 0; j < gds.length; j++) {
          if (String(gds[j].getAttribute("uri") || "") !== pend[k].uri) { continue; }
          /* rid 给了就核关系引用;hostId 给了就核"落在本次分配的那个形状里" */
          if (pend[k].rid && !graphicDataHasRid(gds[j], pend[k].rid)) { continue; }
          if (pend[k].hostId != null && String(gsid[j]) !== String(pend[k].hostId)) { continue; }
          hit = true;
          break;
        }
        if (!hit) {
          return fail("EOFFICE", "本次新增的 a:graphicData 未落在产出里(" + pn + " 找不到 uri="
            + pend[k].uri
            + (pend[k].rid ? " 且内含 r:id/r:embed=" + pend[k].rid : "")
            + (pend[k].hostId != null ? " 且位于 p:cNvPr/@id=" + pend[k].hostId + " 的形状内" : "")
            + (pend[k].rid || pend[k].hostId != null ? "" : "(只按 uri 定位)")
            + " 的 a:graphicData)");
        }
        checked++;
      }
      delete byPart[pn];
    }
    for (k in byPart) {
      if (has(byPart, k)) { return fail("EOFFICE", "本次新增的 a:graphicData 所在部件未被检查 " + k); }
    }
    return { ok: true, checked: checked };
  }
  /* a:graphicData 所属的 pptx 形状(p:spTree 的直接子元素)的 p:cNvPr/@id ——
     没有 pptx 形状(如 docx 的 w:drawing/wp:inline 里的 a:graphic)⇒ 返回 "" */
  function enclosingShapeId(gd) {
    var cur = gd, par, ids;
    while (cur && cur.nodeType === 1) {
      par = cur.parentNode;
      if (par && par.nodeType === 1 && par.namespaceURI === NS.p && par.localName === "spTree") { break; }
      cur = par;
    }
    if (!cur || cur.nodeType !== 1) { return ""; }
    ids = cur.getElementsByTagNameNS(NS.p, "cNvPr");
    if (!ids || !ids.length) { return ""; }
    return String(ids[0].getAttribute("id") || "");
  }
  /* a:graphicData 内是否含指定的关系引用(P2-1 的定位锚:我们写进去的那个节点带的
     r:id / r:embed —— 既有的 SmartArt / OLE 不会命中,因为它们引用的 r:id 都不是
     本次新分配的;图表/表格用 r:id、图片用 r:embed,故两者都认)。
     r:id 是**属性**不是元素(与 rIdsIn 同款写法:getElementsByTagName 不匹配属性) */
  function graphicDataHasRid(gd, rid) {
    var els = gd.getElementsByTagName("*"), i, j, attrs, at;
    for (i = 0; i < els.length; i++) {
      attrs = els[i].attributes;
      if (!attrs) { continue; }
      for (j = 0; j < attrs.length; j++) {
        at = attrs[j];
        if (at.namespaceURI === NS.r && (at.localName === "id" || at.localName === "embed")
          && String(at.value || "") === rid) {
          return true;
        }
      }
    }
    return false;
  }
  /* 对外签名按 §12.8:validatePackage(bytes, fileType);
     opts.requireAll 缺省 false(编辑产物只要求基础件齐 —— 既有文件是别人的);
     opts.addedParts 给"本次新增件"清单(批次 F/H 用);
     opts.addedGraphics 给"本次新增的 a:graphicData"清单(S32 第 ⑦ 条的 uri 白名单作用域,
       条目 = { part, uri, rid };rid = 宿主里指向新增关系的 r:id / r:embed,无关系引用的
       宿主(如表格)可省 —— 此时只按同部件 + 同 uri 定位。缺省 = 不做 uri 扫描) */
  function validatePackage(bytes, fileType, opts) {
    var ft = normFileType(fileType);
    if (!ft) { return fail("EINVAL", "file_type 必须是 docx 或 pptx"); }
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    if (!opts) { opts = { requireAll: false }; }
    return validateEntries(pkg.entries, ft, opts);
  }

  /* ============================================================
     批次 G:页眉页脚(方案 §13;S34 = docx / S35 = pptx)
     ------------------------------------------------------------
     docx(S34,§13.2 的四处):页眉 / 页脚部件 + word/_rels/document.xml.rels 两条关系
     + [Content_Types].xml 两个 Override + w:sectPr 的 w:headerReference /
     w:footerReference。
     **w:sectPr 子元素顺序的 `?` 已由真值件裁决**(§16.2 P-G 的真值包
     shared/tmp/office-tool/groundtruth/gt-hdrftr.docx,Word COM 生成):
     Word 自己写的是「headerReference 与 footerReference 混排」(组内顺序自由)→ `w:pgSz` →
     `w:pgMar` → `w:cols` → `w:titlePg` → `w:docGrid` —— 即 §13.2 ④ 的"引用必须在
     w:pgSz 之前"成立(组内顺序自由,EG_HdrFtrReferences 是 choice 组)。
     本实现按 DOCX_SECTPR_ORDER 顺序表定位插入点(而不是只写死"pgSz 之前"),
     这样 w:titlePg(在 pgMar 之后、docGrid 之前)也落在正确位置。
     `w:pgMar` **完全不动**(§13.2:页眉 / 页脚距离沿用模板值)。
     多节文档只改**末节**(body 级 w:sectPr)并出 warnings。
     pptx(S35,§13.3):pptx 没有页面页眉(header 参数在办公规格层即 EINVAL)。
     页脚文本与幻灯号走**形态 F2**(§13.3 的兜底形态,由 U10 探针定形;探针证据见
     pptxSetHeaderFooter 上方注释):每张幻灯**自带** ftr / sldNum 占位符(几何从版式
     继承),幻灯号段落 = <a:p> 下**直挂** <a:fld type="slidenum">(**与 a:r 同级**,
     不套 a:r 外壳;真值件 gt-footer.pptx 的该段落另带可选的 <a:endParaRPr/>,本实现省略;
     与真值"同形"的部分 = a:p → a:fld 的层级 + p:ph@type/sz/idx 与 a:fld@type 的属性值,
     a:fld@id 是自分配 GUID ⇒ **不是**逐字相同),且**没有任何 <p:hf>**。
     ============================================================ */

  /* CT_SectPr 的子元素顺序(EG_HdrFtrReferences 组 → … → w:pgSz → w:pgMar → … →
     w:titlePg → … → w:docGrid)。表里没有的子元素按"排在最后"处理 ⇒ 插入锚点永远
     落在它之前(新增节点不会跑到已知节点后面) */
  var DOCX_SECTPR_ORDER = [
    "headerReference", "footerReference", "footnotePr", "endnotePr", "type",
    "pgSz", "pgMar", "paperSrc", "pgBorders", "lnNumType", "pgNumType", "cols",
    "formProt", "vAlign", "noEndnote", "titlePg", "textDirection", "bidi",
    "rtlGutter", "docGrid", "printerSettings", "sectPrChange"
  ];
  /* CT_Settings 的顺序表(只列到我们要写的 w:evenAndOddHeaders;其余子元素如
     compat / rsids / themeFontLang / shapeDefaults / decimalSymbol 一律排最后) */
  var DOCX_SETTINGS_ORDER = [
    "writeProtection", "view", "zoom", "removePersonalInformation", "removeDateAndTime",
    "doNotDisplayPageBoundaries", "displayBackgroundShape", "printPostScriptOverText",
    "printFractionalCharacterWidth", "printFormsData", "embedTrueTypeFonts",
    "embedSystemFonts", "saveSubsetFonts", "saveFormsData", "mirrorMargins",
    "alignBordersAndEdges", "bordersDoNotSurroundHeader", "bordersDoNotSurroundFooter",
    "gutterAtTop", "hideSpellingErrors", "hideGrammaticalErrors", "activeWritingStyle",
    "proofState", "formsDesign", "attachedTemplate", "linkStyles",
    "stylePaneFormatFilter", "stylePaneSortMethod", "documentType", "mailMerge",
    "revisionView", "trackChanges", "doNotTrackMoves", "doNotTrackFormatting",
    "documentProtection", "autoFormatOverride", "styleLockTheme", "styleLockQFSet",
    "defaultTabStop", "autoHyphenation", "consecutiveHyphenLimit", "hyphenationZone",
    "doNotHyphenateCaps", "showEnvelope", "summaryLength", "clickAndTypeStyle",
    "defaultTableStyle", "evenAndOddHeaders"
  ];
  function orderIndexOf(list, local) {
    for (var i = 0; i < list.length; i++) { if (list[i] === local) { return i; } }
    return list.length;   /* 未知元素 = 排在最后 */
  }
  /* 按顺序表插入:找第一个"序数 > 自己"的同命名空间兄弟 ⇒ 插它前面;没有则 append */
  function insertByOrder(parent, el, list) {
    var want = orderIndexOf(list, el.localName), kids = parent.childNodes, i, n;
    for (i = 0; i < kids.length; i++) {
      n = kids[i];
      if (n.nodeType !== 1 || n.namespaceURI !== el.namespaceURI) { continue; }
      if (orderIndexOf(list, n.localName) > want) { parent.insertBefore(el, n); return; }
    }
    parent.appendChild(el);
  }
  /* 页眉 / 页脚 / settings 部件名(单一命名源:写侧与读侧都从这里取) */
  function headerPart(n) { return "word/header" + n + ".xml"; }
  function footerPart(n) { return "word/footer" + n + ".xml"; }
  function hfPartOf(kind, n) { return (kind === "header" ? headerPart(n) : footerPart(n)); }
  /* 下一个可用序号:扫包内已有的 word/headerN.xml(不靠外部计数,重复调用也不会撞名) */
  function nextHfIndex(entries, kind) {
    var re = new RegExp("^word/" + kind + "([0-9]+)\\.xml$"), n = 0, k, m;
    for (k in entries) {
      if (!has(entries, k)) { continue; }
      m = re.exec(k);
      if (m && parseInt(m[1], 10) > n) { n = parseInt(m[1], 10); }
    }
    return n + 1;
  }
  /* 确保某个 rels 里有 (Type + Target) 这条关系(缺则追加 + 用未占用的 rId)。
     与 relsEnsure 的差别:判据是 **Type 与 Target 都相等** —— 页眉 / 页脚的关系类型
     在同一个 rels 里出现多次(header1 / header2 / header3 同 Type),relsEnsure 的
     "同 Type 即命中"会把第二条当成已存在。返回 rid(命中也要回 rid,宿主可能还没引用) */
  function relsEnsureTarget(entries, relsPart, type, target, fallback) {
    var raw = partText(entries, relsPart), fromTpl = false;
    if (raw == null) {
      if (fallback == null || relsPart !== PART.documentRels) {
        return fail("EOFFICE", "缺少部件 " + relsPart);
      }
      raw = fallback; fromTpl = true;      /* §2.4 的按需件:docx 的 document.xml.rels */
    }
    var par = xmlParse(raw);
    if (!par.ok) { return fail(par.code, relsPart + " 解析失败:" + par.error); }
    var doc = par.doc, root = doc.documentElement;
    var rels = root.getElementsByTagNameNS(NS.pr, "Relationship");
    var max = 0, i, id, m, ty, tg;
    for (i = 0; i < rels.length; i++) {
      ty = String(rels[i].getAttribute("Type") || "");
      tg = String(rels[i].getAttribute("Target") || "");
      if (ty === type && tg === target) {
        return { ok: true, rid: String(rels[i].getAttribute("Id") || ""),
          text: fromTpl ? raw : undefined };
      }
      id = String(rels[i].getAttribute("Id") || "");
      m = /^rId([0-9]+)$/.exec(id);
      if (m && parseInt(m[1], 10) > max) { max = parseInt(m[1], 10); }
    }
    var rid = "rId" + (max + 1);
    var rel = elNs(doc, NS.pr, "Relationship");
    rel.setAttribute("Id", rid);
    rel.setAttribute("Type", type);
    rel.setAttribute("Target", target);
    root.appendChild(rel);
    var s = xmlSerialize(doc, fromTpl ? XML_DECL_STD : xmlDeclOf(raw));
    if (!s.ok) { return s; }
    return { ok: true, rid: rid, text: s.text };
  }
  /* 把页眉 / 页脚部件的文本整体设成 text(空串 ⇒ 一个空 w:p):
     只动根元素直接子的 w:p —— 第一个保留(清掉它的非 w:pPr 子元素,再挂一个新 w:r),
     其余 w:p 删掉(该部件的文本由本次设置决定,留旧段会拼出"旧文 + 新文") */
  function docxHfApplyText(doc, root, text) {
    var kids = root.childNodes, ps = [], i, j, n, p;
    for (i = 0; i < kids.length; i++) {
      n = kids[i];
      if (n.nodeType === 1 && n.namespaceURI === NS.w && n.localName === "p") { ps.push(n); }
    }
    p = ps.length ? ps[0] : null;
    if (!p) { p = elW(doc, "p"); root.appendChild(p); }
    for (i = ps.length - 1; i >= 1; i--) { root.removeChild(ps[i]); }
    var ck = p.childNodes;
    for (j = ck.length - 1; j >= 0; j--) {
      n = ck[j];
      if (n.nodeType === 1 && n.namespaceURI === NS.w && n.localName !== "pPr") { p.removeChild(n); }
    }
    var t = String(text == null ? "" : text);
    if (t.length) { p.appendChild(wRun(doc, t, false, false)); }
    return { ok: true, runs: t.length ? 1 : 0 };
  }
  /* 某个 sectPr 里 w:<kind>Reference 且 w:type == ty 的那一条(ty 缺省 = default) */
  function sectRefOf(sectPr, kind, ty) {
    var refs = sectPr.getElementsByTagNameNS(NS.w, kind + "Reference"), i;
    for (i = 0; i < refs.length; i++) {
      if (String(refs[i].getAttributeNS(NS.w, "type") || "default") === ty) { return refs[i]; }
    }
    return null;
  }
  /* 一条引用的目标部件(经 document.xml.rels;找不到 → null) */
  function sectRefTarget(entries, ref) {
    if (!ref) { return null; }
    var rid = ref.getAttributeNS(NS.r, "id");
    if (!rid) { return null; }
    var rm = relsMapOf(entries, PART.documentRels);
    if (!rm.ok) { return null; }
    var rel = rm.map[rid];
    if (!rel || !rel.target) { return null; }
    return resolvePartName("word", rel.target);
  }
  /* 建一个页眉 / 页脚部件(n 由调用方给,调用方负责 rels / CT / 引用三处同步) */
  function hfPartBuild(kind, text) {
    var t = tplPartOf(TPL_DOCX, "docx", kind, function (doc) {
      return docxHfApplyText(doc, doc.documentElement, text);
    });
    if (!t.ok) { return t; }
    return { ok: true, text: t.text };
  }
  /* S34:set_header_footer(docx) */
  function docxSetHeaderFooter(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var wants = [], i, kind, hasHeader = (spec.header != null), hasFooter = (spec.footer != null);
    if (hasHeader) { wants.push({ kind: "header", text: String((spec.header && spec.header.text) || "") }); }
    if (hasFooter) { wants.push({ kind: "footer", text: String((spec.footer && spec.footer.text) || "") }); }
    if (!wants.length) {
      return fail("EINVAL", "set_header_footer 至少要给 header 或 footer 里的一个");
    }
    var first = (spec.first_page_different === true), even = (spec.even_odd === true);
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var d = docxPartOf(pkg.entries, PART.document);
    if (!d.ok) { return d; }
    var doc = d.doc, body = docxBody(doc);
    if (!body) { return fail("EOFFICE", PART.document + " 缺少 w:body"); }
    var sectPr = docxBodySectPr(body);
    if (!sectPr) { return fail("EOFFICE", PART.document + " 的 w:body 里没有 w:sectPr"); }
    progress(opts, "build");
    var entries = pkg.entries;
    var writes = {}, addedParts = [], warns = [], steps = [], changed = 0, docTouched = false;
    var sectCount = (doc.getElementsByTagNameNS(NS.w, "sectPr") || []).length;
    if (sectCount > 1) {
      warns.push("检测到 " + sectCount + " 个节,页眉 / 页脚只应用到末节");
    }
    /* 一次调用里要确保的部件集合:默认一套 + (首页不同)首页一套 + (奇偶不同)偶数页一套 */
    var jobs = [], j;
    for (i = 0; i < wants.length; i++) {
      jobs.push({ kind: wants[i].kind, ty: "default", text: wants[i].text });
    }
    if (first) {
      for (i = 0; i < wants.length; i++) { jobs.push({ kind: wants[i].kind, ty: "first", text: "" }); }
    }
    if (even) {
      for (i = 0; i < wants.length; i++) { jobs.push({ kind: wants[i].kind, ty: "even", text: "" }); }
    }
    for (j = 0; j < jobs.length; j++) {
      var job = jobs[j];
      kind = job.kind;
      var part = sectRefTarget(entries, sectRefOf(sectPr, kind, job.ty));
      if (part && has(entries, part)) {
        /* ① 已有引用且部件在:同值 ⇒ 一个字节都不动(V12 的幂等) */
        var cur = partText(entries, part);
        var curPar = xmlParse(cur);
        if (!curPar.ok) { return fail(curPar.code, part + " 解析失败:" + curPar.error); }
        var curText = wTextsOf(curPar.doc.documentElement);
        if (curText === job.text) { continue; }
        var m = docxHfApplyText(curPar.doc, curPar.doc.documentElement, job.text);
        if (!m.ok) { return m; }
        var ser = xmlSerialize(curPar.doc, xmlDeclOf(cur));
        if (!ser.ok) { return ser; }
        writes[part] = ser.text;
        changed++;
        continue;
      }
      /* ② 没有引用(或引用指向的部件不在):新建部件 + 关系 + CT Override + sectPr 引用 */
      var n = nextHfIndex(entries, kind);
      var newPart = hfPartOf(kind, n);
      var built = hfPartBuild(kind, job.text);
      if (!built.ok) { return built; }
      var rel = relsEnsureTarget(entries, PART.documentRels, REL_DOC_RELS + kind,
        kind + n + ".xml", TPL_DOCX.documentRels);
      if (!rel.ok) { return rel; }
      var ct = ctEnsureOverride(entries, newPart, kind === "header" ? CT_DOCX.header : CT_DOCX.footer);
      if (!ct.ok) { return ct; }
      var ref = elW(doc, kind + "Reference");
      wAttr(ref, "type", job.ty);
      rAttr(ref, "id", rel.rid);
      insertByOrder(sectPr, ref, DOCX_SECTPR_ORDER);
      writes[newPart] = built.text;
      if (rel.text != null) { writes[PART.documentRels] = rel.text; }
      if (ct.text != null) { writes[PART.contentTypes] = ct.text; }
      entries[newPart] = textToBytes(built.text);
      if (rel.text != null) { entries[PART.documentRels] = textToBytes(rel.text); }
      if (ct.text != null) { entries[PART.contentTypes] = textToBytes(ct.text); }
      addedParts.push(newPart);
      changed++;
      docTouched = true;
      steps.push("新建 " + newPart + " + 关系 " + rel.rid + " + Content_Types Override + "
        + kind + "Reference(w:type=\"" + job.ty + "\")");
    }
    /* 首页不同 ⇒ w:titlePg;奇偶不同 ⇒ word/settings.xml 的 w:evenAndOddHeaders */
    if (first && !hasSectChild(sectPr, "titlePg")) {
      insertByOrder(sectPr, elW(doc, "titlePg"), DOCX_SECTPR_ORDER);
      docTouched = true;
      steps.push("w:sectPr 加 w:titlePg(首页不同)");
      warns.push("已开启首页不同:首页页眉 / 页脚为空(本 operation 没有「首页文本」参数)");
    }
    if (even) {
      var sp = partText(entries, PART.settings);
      var spNew = (sp == null);
      var sraw = spNew ? TPL_DOCX.settings : sp;
      var spar = xmlParse(sraw);
      if (!spar.ok) { return fail(spar.code, PART.settings + " 解析失败:" + spar.error); }
      if (!hasSectChild(spar.doc.documentElement, "evenAndOddHeaders")) {
        insertByOrder(spar.doc.documentElement, elW(spar.doc, "evenAndOddHeaders"), DOCX_SETTINGS_ORDER);
      }
      var sser = xmlSerialize(spar.doc, xmlDeclOf(sraw));
      if (!sser.ok) { return sser; }
      writes[PART.settings] = sser.text;
      entries[PART.settings] = textToBytes(sser.text);
      if (spNew) {
        var sct = ctEnsureOverride(entries, PART.settings, CT_DOCX.settings);
        if (!sct.ok) { return sct; }
        var srel = relsEnsureTarget(entries, PART.documentRels, REL_SETTINGS, "settings.xml",
          TPL_DOCX.documentRels);
        if (!srel.ok) { return srel; }
        if (sct.text != null) { writes[PART.contentTypes] = sct.text; entries[PART.contentTypes] = textToBytes(sct.text); }
        if (srel.text != null) { writes[PART.documentRels] = srel.text; entries[PART.documentRels] = textToBytes(srel.text); }
        addedParts.push(PART.settings);
        steps.push("新建 " + PART.settings + "(w:evenAndOddHeaders)+ CT Override + settings 关系");
      } else {
        steps.push("已有 " + PART.settings + ":补 w:evenAndOddHeaders");
      }
      changed++;
      warns.push("已开启奇偶不同:偶数页页眉 / 页脚为空(本 operation 没有「偶数页文本」参数),"
        + "默认文本应用于奇数页");
    }
    if (docTouched) {
      var ds = xmlSerialize(doc, d.decl);
      if (!ds.ok) { return ds; }
      writes[PART.document] = ds.text;
    }
    if(!changed) {
      /* 同值重设:原样回传输入字节(V12 —— 目标部件逐字节不变,连容器都不重打) */
      var u8 = asBytes(bytes);
      return {
        ok: true, bytes: u8, mime: MIME.docx, size: u8.length, unchanged: true,
        counts: { headerFooter: 0, sections: sectCount }, warnings: warns,
        steps: ["页眉 / 页脚已是目标文本,未改动任何部件"]
      };
    }
    var fin = finishDraft(pkg, writes, "docx", false, opts, addedParts);
    if (!fin.ok) { return fin; }
    if (!steps.length) { steps.push("改写页眉 / 页脚文本(" + changed + " 个部件)"); }
    progress(opts, "done");
    return okResult(fin.bytes, "docx", { headerFooter: changed, sections: sectCount }, warns, steps);
  }
  /* sectPr / settings 里有没有某个直接子元素 */
  function hasSectChild(parent, local) {
    var kids = parent.childNodes, i, n;
    for (i = 0; i < kids.length; i++) {
      n = kids[i];
      if (n.nodeType === 1 && n.namespaceURI === NS.w && n.localName === local) { return true; }
    }
    return false;
  }

  /* ---------------- S35:set_header_footer(pptx) ---------------- */
  var SLIDENUM_GUID = "{2E4E5F1A-3F3B-4D0A-9C6E-5A7B8C9D0E1F}";
  /* 幻灯号占位符的段落(真值口径:a:p 下**直挂** a:fld type="slidenum")。缓存文本:
     幻灯自己的占位符用它自己的序号(`1` / `2` …),母版 / 版式的定义件用真值默认的 `‹#›`。
     ⚠ a:fld 必须与 a:r **同级**挂进 a:p —— 不得拿 a:r 当外壳:DrawingML 的
     a:p = (a:pPr?, EG_TextRun*, a:endParaRPr?)、EG_TextRun = a:r | a:br | a:fld,
     而 a:r(CT_RegularTextRun)只许 (a:rPr?, a:t) ⇒ 套在 a:r 里是非法结构,
     PowerPoint 据此判整包损坏(0x80070570;批 G 的 P0,见 progress 的差分实验)。 */
  function pptxSldNumPara(doc, txt) {
    var p = elA(doc, "p");
    var fld = elA(doc, "fld");
    nAttr(fld, "id", SLIDENUM_GUID);
    nAttr(fld, "type", "slidenum");
    var rPr = elA(doc, "rPr");
    nAttr(rPr, "lang", "en-US");
    nAttr(rPr, "smtClean", "0");
    fld.appendChild(rPr);
    fld.appendChild(aText(doc, txt == null ? "\u2039#\u203A" : String(txt)));
    p.appendChild(fld);
    return p;
  }
  /* 把一个 p:sp(占位符)的文本框整体换成 lines(保留 spPr 几何与占位符属性) */
  function pptxSetSpLines(doc, sp, lines) {
    var tb = pptxTxBodyOf(sp), i;
    if (!tb) { return { ok: false, error: "占位符没有 p:txBody" }; }
    var arr = (lines && lines.length) ? lines : [""];
    pptxClearParas(tb);
    for (i = 0; i < arr.length; i++) { tb.appendChild(pptxPara(doc, arr[i])); }
    return { ok: true };
  }
  /* 清掉某个 p:txBody 下的全部 a:p */
  function pptxClearParas(tb) {
    var kids = tb.childNodes, i, n;
    for (i = kids.length - 1; i >= 0; i--) {
      n = kids[i];
      if (n.nodeType === 1 && n.namespaceURI === NS.a && n.localName === "p") { tb.removeChild(n); }
    }
  }
  /* 形状里有 <a:fld type="slidenum"> 就算"编号已就位" —— 再写一次会换 GUID,
     破坏 §13.4 / V12 要求的幂等 */
  function pptxHasSldNumField(sp) {
    var fs = sp.getElementsByTagNameNS(NS.a, "fld"), i;
    for (i = 0; i < fs.length; i++) {
      if (String(fs[i].getAttribute("type") || "") === "slidenum") { return true; }
    }
    return false;
  }
  /* 幻灯自己的版式部件(经 slideN.xml.rels 的 slideLayout 关系;读不到 → null) */
  function pptxSlideLayoutOf(entries, relsPart) {
    var rm = relsMapOf(entries, relsPart), k;
    if (!rm.ok) { return null; }
    for (k in rm.map) {
      if (has(rm.map, k) && rm.map[k].type === REL_SLIDE_LAYOUT) {
        return resolvePartName("ppt/slides", rm.map[k].target);
      }
    }
    return null;
  }
  /* 一个占位符形状的 a:xfrm 读数(EMU;没有 a:xfrm → null) */
  function pptxSpXfrmOf(sp) {
    var xs = sp.getElementsByTagNameNS(NS.a, "xfrm"), off, ext, r;
    if (!xs || !xs.length) { return null; }
    off = xs[0].getElementsByTagNameNS(NS.a, "off");
    ext = xs[0].getElementsByTagNameNS(NS.a, "ext");
    if (!off || !off.length || !ext || !ext.length) { return null; }
    r = { x: off[0].getAttribute("x"), y: off[0].getAttribute("y"),
      cx: ext[0].getAttribute("cx"), cy: ext[0].getAttribute("cy") };
    if (r.x == null || r.y == null || r.cx == null || r.cy == null) { return null; }
    return r;
  }
  /* ftr / sldNum 占位符的定义:先在幻灯**自己的版式**里找(真值形态 —— 幻灯级占位符
     抄版式的 idx,几何由版式继承);找不到退到母版,此时几何无同 idx 的版式可继承 ⇒
     连母版那个占位符的 a:xfrm 一起带下来(自包含)。返回 {ph, from, xfrm} 或 null */
  function pptxHfPhDef(entries, layPart, phType) {
    var jobs = [], i, j, d, shapes, ph;
    if (layPart) { jobs.push({ part: layPart, from: "layout" }); }
    jobs.push({ part: PART.slideMaster, from: "master" });
    for (i = 0; i < jobs.length; i++) {
      d = partDocOf(entries, jobs[i].part);
      if (!d.ok) { continue; }
      shapes = pptxShapesOf(d.doc);
      for (j = 0; j < shapes.length; j++) {
        ph = pptxPhOf(shapes[j]);
        if (ph && pptxPhType(ph) === phType) {
          return { ph: ph, from: jobs[i].from,
            xfrm: (jobs[i].from === "layout") ? null : pptxSpXfrmOf(shapes[j]) };
        }
      }
    }
    return null;
  }
  /* 幻灯自己 p:spTree 里的某个占位符形状(直接子形状口径,不递归) */
  function pptxSlideHfShape(doc, phType) {
    var shapes = pptxShapesOf(doc), i, ph;
    for (i = 0; i < shapes.length; i++) {
      ph = pptxPhOf(shapes[i]);
      if (ph && pptxPhType(ph) === phType) { return shapes[i]; }
    }
    return null;
  }
  /* 在**一张幻灯**上把某个占位符设成目标状态(§13.3 F2)。
     task = {mode:"text", text} | {mode:"num", n} | {mode:"remove"};def 只在新建时用。
     同值 / 已就位 ⇒ changed:false(§13.4 最小改动 + V12 幂等)。
     "关掉"(remove)的语义 = 把幻灯级占位符移掉 —— 真值口径:PowerPoint 只把"开着"
     的页脚 / 编号落到幻灯上,没落 = 不显示 */
  function pptxSlideHfApply(doc, phType, def, task) {
    var cur = pptxSlideHfShape(doc, phType), tb, m, sp, spPr, tree, id, nm;
    if (task.mode === "remove") {
      if (!cur || !cur.parentNode) { return { ok: true, changed: false }; }
      cur.parentNode.removeChild(cur);
      return { ok: true, changed: true };
    }
    if (cur) {
      if (task.mode === "num") {
        if (pptxHasSldNumField(cur)) { return { ok: true, changed: false }; }
        tb = pptxTxBodyOf(cur);
        if (!tb) { return { ok: true, changed: false, reason: "占位符没有 p:txBody" }; }
        pptxClearParas(tb);
        tb.appendChild(pptxSldNumPara(doc, task.n));
        return { ok: true, changed: true };
      }
      if (pptxTextOf(cur) === task.text) { return { ok: true, changed: false }; }
      m = pptxSetSpLines(doc, cur, [task.text]);
      return { ok: true, changed: !!m.ok, reason: m.ok ? null : m.error };
    }
    if (!def) { return { ok: true, changed: false, reason: "版式与母版都没有 " + phType + " 占位符" }; }
    tree = pptxSpTree(doc);
    if (!tree) { return { ok: true, changed: false, reason: "缺 p:spTree" }; }
    id = nextShapeId(doc);
    nm = (phType === "ftr" ? "Footer Placeholder " : "Slide Number Placeholder ") + (id - 1);
    sp = pptxSlideSp(doc, def.ph, id, nm, task.mode === "num" ? [] : [task.text]);
    if (def.xfrm) {
      spPr = sp.getElementsByTagNameNS(NS.p, "spPr");
      if (spPr && spPr.length) { spPr[0].appendChild(pptxXfrm(doc, def.xfrm)); }
    }
    if (task.mode === "num") {
      tb = pptxTxBodyOf(sp);
      if (tb) { pptxClearParas(tb); tb.appendChild(pptxSldNumPara(doc, task.n)); }
    }
    tree.appendChild(sp);
    return { ok: true, changed: true, added: true };
  }
  /* S35:set_header_footer(pptx) —— 页脚文本 + 幻灯号(§13.3 形态 F2;U10 探针定形)
     ------------------------------------------------------------------
     **形态裁定:F2**(方案 §13.3 的兜底形态;探针证据见下)。写入面 = 每张幻灯自己的
     ftr / sldNum 占位符(其 p:spPr 为空 ⇒ 几何从版式继承),幻灯号段落 =
     <a:p> 下**直挂** <a:fld type="slidenum">(**与 a:r 同级**,不套 a:r 外壳)——
     层级与 PowerPoint 自己写的 gt-footer.pptx 同形(该真值件另带可选的 <a:endParaRPr/>,
     本实现省略;a:fld@id 是自分配 GUID,故非逐字相同),且**全程不写 <p:hf>**。

     F1(文本只写母版 + 版式、每张幻灯加 <p:hf ftr="1" sldNum="1"/>)已被探针**否决**:
       ① schema:CT_Slide 的子元素只有 cSld / EG_ChildSlide(clrMapOvr)/ transition /
          timing / extLst,**没有 hf** —— hf(CT_HeaderFooter)是 p:sldMaster /
          p:notesMaster / p:handoutMaster 的子元素(c-rex OOXML 文档 §CT_Slide);
       ② COM 实测(shared/tmp/office-tool/g/probe-ppt.ps1):把 gt-footer.pptx 改造成 F1
          (去掉幻灯级占位符、文本写进版式 + 母版、每张加 <p:hf>)后,PowerPoint 读回来是
          slide 1/2 `Footer.Text=[] Footer.Visible=0 SlideNumber.Visible=0`、`Shapes=1`,
          而同一个探针读 gt-footer.pptx(F2)是 `Footer.Text=[GT PPT FOOTER]
          Footer.Visible=-1 SlideNumber.Visible=-1 Shapes=3` ⇒ F1 的继承**不生效**;
       ③ F1 件被 PowerPoint 转存时 `<p:hf>` 被**静默丢弃**(probe-f1-rt.pptx 里 p:hf 消失),
          而 F2 件转存后页脚 / 编号占位符原样保留;
       ④ 全库 70 份 pptx 扫描:`<p:hf>` 出现 0 次。
     故 T14 里 pptx 那句"母版 + 两个版式的占位符有文本、每张 slide 有 p:hf"按真值改为
     "每张 slide 自带 ftr / sldNum 占位符并有文本"。
     幂等(V12):同值重设 ⇒ 一个字节都不变(ftr 比文本、sldNum 认 <a:fld>/GUID 相同;
     母版 / 版式**全程不碰**)。 */
  function pptxSetHeaderFooter(bytes, spec, opts) {
    var abort = stopIfAborted(opts);
    if (abort) { return abort; }
    var hasFtr = (spec.footer != null);
    var hasNum = (spec.slide_number != null);
    if (!hasFtr && !hasNum) {
      return fail("EINVAL", "set_header_footer(pptx)至少要给 footer 或 slide_number");
    }
    progress(opts, "parse");
    var pkg = loadDraft(bytes);
    if (!pkg.ok) { return pkg; }
    var entries = pkg.entries;
    var sl = pptxSlideList(entries);
    if (!sl.ok) { return sl; }
    progress(opts, "build");
    var writes = {}, warns = [], steps = [], changed = 0, i;
    var ftrText = hasFtr ? String((spec.footer && spec.footer.text) || "") : null;
    var wantsNum = hasNum ? (spec.slide_number === true) : false;
    var addFtr = 0, upFtr = 0, rmFtr = 0, addNum = 0, upNum = 0, rmNum = 0, noDef = 0;
    var defCache = {};
    for (i = 0; i < sl.slides.length; i++) {
      var ent = sl.slides[i];
      var sd = partDocOf(entries, ent.part);
      if (!sd.ok) { return sd; }
      var doc = sd.doc, touch = false, r, key;
      var layPart = pptxSlideLayoutOf(entries, ent.relsPart);
      if (hasFtr) {
        if (!ftrText.length) {
          r = pptxSlideHfApply(doc, "ftr", null, { mode: "remove" });
          if (r.changed) { rmFtr++; touch = true; }
        } else {
          key = "ftr|" + (layPart || "");
          if (!has(defCache, key)) { defCache[key] = pptxHfPhDef(entries, layPart, "ftr"); }
          if (!defCache[key]) { noDef++; }
          r = pptxSlideHfApply(doc, "ftr", defCache[key], { mode: "text", text: ftrText });
          if (r.changed) {
            touch = true;
            if (r.added) { addFtr++; } else { upFtr++; }
          }
        }
      }
      if (hasNum) {
        if (!wantsNum) {
          r = pptxSlideHfApply(doc, "sldNum", null, { mode: "remove" });
          if (r.changed) { rmNum++; touch = true; }
        } else {
          key = "sldNum|" + (layPart || "");
          if (!has(defCache, key)) { defCache[key] = pptxHfPhDef(entries, layPart, "sldNum"); }
          if (!defCache[key]) { noDef++; }
          r = pptxSlideHfApply(doc, "sldNum", defCache[key], { mode: "num", n: i + 1 });
          if (r.changed) {
            touch = true;
            if (r.added) { addNum++; } else { upNum++; }
          }
        }
      }
      if (!touch) { continue; }
      var ss = xmlSerialize(doc, sd.decl);
      if (!ss.ok) { return ss; }
      writes[ent.part] = ss.text;
      changed++;
    }
    if (noDef) {
      warns.push("有 " + noDef + " 处幻灯的版式与母版都没有目标占位符,已跳过该处");
    }
    if (addFtr || upFtr || rmFtr) {
      steps.push("每张幻灯的 ftr 占位符:新建 " + addFtr + " / 改文本 " + upFtr + " / 移除 " + rmFtr);
    }
    if (addNum || upNum || rmNum) {
      steps.push("每张幻灯的 sldNum 占位符:新建 " + addNum + " / 写入 a:fld type=slidenum "
        + upNum + " / 移除 " + rmNum);
    }
    if (hasFtr && !ftrText.length) {
      steps.push("footer.text 为空串 ⇒ 移除幻灯级 ftr 占位符(等于不显示页脚)");
    }
    if (hasNum && !wantsNum) { steps.push("slide_number=false ⇒ 移除幻灯级 sldNum 占位符"); }
    if (!changed) {
      var u8 = asBytes(bytes);
      return {
        ok: true, bytes: u8, mime: MIME.pptx, size: u8.length, unchanged: true,
        counts: { headerFooter: 0, slides: sl.slides.length }, warnings: warns,
        steps: ["页脚 / 幻灯号已是目标值,未改动任何部件"]
      };
    }
    var fin = finishDraft(pkg, writes, "pptx", false, opts);
    if (!fin.ok) { return fin; }
    progress(opts, "done");
    return okResult(fin.bytes, "pptx", { headerFooter: changed, slides: sl.slides.length },
      warns, steps);
  }

  /* ============================================================
     入口分派(工具面接线在批次 D:此刻**不得**把 office-tool 注册进
     AGENT_TOOLS —— 注册了模型会拿到 EOFFICE)
     spec 的契约字段 = { op, fileType, ..., props }(方案 §2.3);operation 作为别名
     一并容忍(归一化实现可能用后者)。opts = { signal, onProgress }。
     批次 C 起 docx / pptx 两套都在:`file_type` 决定 op 落到哪一套(方案 §2.2 的
     operation × file_type 两张子表在 OfficeWrite 层做**粗分派**,参数作用域的
     逐条判据仍在批次 D 的 officeSpecCheck —— 这里只在"该 file_type 根本没有这个 op"
     时报 EINVAL)。
     ============================================================ */
  function officeRun(input, spec, opts) {
    var s = spec || {};
    var op = String(s.op || s.operation || "");
    var ft = normFileType(s.fileType || s.file_type);
    var bytes = input ? input.bytes : null;
    if (!op) { return fail("EINVAL", "缺少 op(operation)"); }
    if (op === "create") {
      if (!ft) { return fail("EINVAL", "create 必须给 file_type(docx 或 pptx)"); }
      return (ft === "pptx") ? pptxCreate(s, opts) : docxCreate(s, opts);
    }
    if (!ft) { return fail("EINVAL", "编辑类 operation 必须给 file_type(docx 或 pptx)"); }
    if (op === "outline") {
      return (ft === "pptx") ? pptxOutline(bytes, s, opts) : docxOutline(bytes, s, opts);
    }
    if (ft === "pptx") {
      if (op === "set_text") { return pptxSetText(bytes, s, opts); }
      if (op === "add_slide") { return pptxAddSlide(bytes, s, opts); }
      if (op === "replace_text") { return pptxReplaceText(bytes, s, opts); }
      if (op === "delete") { return pptxDelete(bytes, s, opts); }
      if (op === "set_properties") { return pptxSetProperties(bytes, s, opts); }
      if (op === "add_chart") { return pptxAddChart(bytes, s, opts); }
      if (op === "set_header_footer") { return pptxSetHeaderFooter(bytes, s, opts); }
      if (op === "add_table") { return pptxAddTable(bytes, s, opts); }
      if (op === "add_image") { return pptxAddImage(bytes, s, opts); }
      if (op === "apply_layout") { return pptxApplyLayout(bytes, s, opts); }
      return fail("EINVAL", "未知 operation:" + op
        + "(pptx 支持 create / outline / add_slide / replace_text / set_text / delete / set_properties"
        + " / add_chart / set_header_footer / add_table / add_image / apply_layout)");
    }
    if (op === "append") { return docxAppend(bytes, s, opts); }
    if (op === "replace_text") { return docxReplaceText(bytes, s, opts); }
    if (op === "delete") { return docxDelete(bytes, s, opts); }
    if (op === "set_properties") { return docxSetProperties(bytes, s, opts); }
    if (op === "add_chart") { return docxAddChart(bytes, s, opts); }
    if (op === "set_header_footer") { return docxSetHeaderFooter(bytes, s, opts); }
    if (op === "add_table") { return docxAddTable(bytes, s, opts); }
    if (op === "add_image") { return docxAddImage(bytes, s, opts); }
    return fail("EINVAL", "未知 operation:" + op
      + "(docx 支持 create / outline / append / replace_text / delete / set_properties"
      + " / add_chart / set_header_footer / add_table / add_image;apply_layout 只对 pptx 有意义)");
  }

  /* ---------------- 能力自检 ---------------- */
  function missingCaps() {
    var miss = [];
    if (typeof W.Promise !== "function") { miss.push("Promise"); }
    if (typeof W.Uint8Array !== "function") { miss.push("Uint8Array"); }
    if (typeof W.Blob !== "function") { miss.push("Blob"); }
    if (typeof W.TextDecoder !== "function") { miss.push("TextDecoder"); }
    if (typeof W.TextEncoder !== "function") { miss.push("TextEncoder"); }
    if (typeof W.DOMParser !== "function") { miss.push("DOMParser"); }
    if (typeof W.XMLSerializer !== "function") { miss.push("XMLSerializer"); }
    if (typeof W.btoa !== "function") { miss.push("btoa"); }
    return miss;
  }
  /* 载荷自检:存在 + 可编译(fflateOf 的取用是懒加载,这里只做一次廉价编译探针) */
  function payloadProbe() {
    var src = payloadText(FFLATE_ID);
    if (!src) {
      return "缺少内嵌载荷: " + FFLATE_ID + "(请跑 node scripts/make-office-part.js 重新生成 src/office.part)";
    }
    try {
      /* eslint-disable-next-line no-new-func */
      new Function(src);
    } catch (e) {
      return "内嵌载荷无法编译: " + FFLATE_ID + "(src/office.part 可能已损坏,请重新生成)";
    }
    return "";
  }

  /* ---------------- 对外入口 ----------------
     run(input, spec, opts) —— 批次 A 落 ZIP / XML 地基,批次 B 落 docx 生成与编辑,
     批次 C 落 pptx 生成与编辑(S13–S18),批次 F 落图表 + 内嵌工作簿(S28–S32),
     批次 G 落页眉页脚(S34 的 docx / S35 的 pptx),批次 H 落表格 / 图片 / 套用版式
     (S37 / S38 / S39;S40 已取消)。
     契约(方案 §2.3):参数 input={bytes} / spec={op,fileType,...,props} / opts={signal,onProgress};
     永不 reject,失败 { ok:false, code, error }。 */
  function run(input, spec, opts) {
    if (!api.available) {
      return W.Promise.resolve(fail("EOFFICE", "写入核心不可用:" + (api.why || "未知原因")));
    }
    var res;
    try {
      res = officeRun(input, spec, opts);
    } catch (e) {
      res = fail(codeOf(e), msgOf(e));
    }
    return W.Promise.resolve(res);
  }
  function diag() {
    return { version: VERSION, limits: api.limits, mode: api.mode };
  }

  var api = {
    available: false,
    why: "",
    version: VERSION,
    formats: FORMATS.slice(),
    limits: LIMITS,
    mode: "none",
    diag: diag,
    run: run,
    /* 验收钩子:暴露内部各层,供无头脚本在**真产物**上直接对拍(批次 A 的 S6 四条断言
       要直呼 ZIP / XML;批次 B 的 A16 负例要直呼 validatePackage / 各 builder;
       批次 C 要读 TPL_PPTX 与 pptx 结构层)。
       它挂在 OfficeWrite 命名空间内、不新增全局、不发起任何请求;
       对外契约仍是上面 7 个字段 */
    _layers: {
      zipPrescan: zipPrescan, zipUnzip: zipUnzip, zipBuild: zipBuild,
      xmlParse: xmlParse, xmlDeclOf: xmlDeclOf, xmlSerialize: xmlSerialize, xmlText: xmlText,
      fflateOf: fflateOf, bytesToText: bytesToText, textToBytes: textToBytes, NS: NS, PART: PART,
      validatePackage: validatePackage, validateEntries: validateEntries,
      parseContentLite: parseContentLite, parseInline: parseInline,
      docxOutlineOf: docxOutlineOf, docxItems: docxItems, docxPageOf: docxPageOf,
      docxHeaderFooter: docxHeaderFooter, wTextsOf: wTextsOf,
      /* 批次 C:pptx */
      TPL_PPTX: TPL_PPTX, CT_PPTX: CT_PPTX, PPTX_LAYOUT_KEYS: PPTX_LAYOUT_KEYS,
      slidePart: slidePart, slideRelsPart: slideRelsPart, relTargetFrom: relTargetFrom,
      partDocOf: partDocOf, relsMapOf: relsMapOf, mutatePart: mutatePart,
      ctEnsureOverride: ctEnsureOverride, ctRemoveOverride: ctRemoveOverride,
      setAppSlides: setAppSlides, pptxSlideList: pptxSlideList, pptxLayoutsOf: pptxLayoutsOf,
      pptxShapesOf: pptxShapesOf, pptxKindOf: pptxKindOf, pptxPhOf: pptxPhOf,
      pptxPhType: pptxPhType, pptxPhIdx: pptxPhIdx, pptxTextOf: pptxTextOf,
      pptxTextPhsOfDoc: pptxTextPhsOfDoc, pptxFtrOf: pptxFtrOf, pptxHfFlag: pptxHfFlag,
      pptxIsOwnDeck: pptxIsOwnDeck, pptxTxBodyOf: pptxTxBodyOf,
      pptxSlideXml: pptxSlideXml, pptxSlideRelsText: pptxSlideRelsText,
      pptxOutlineOf: pptxOutlineOf, nextSlideIndex: nextSlideIndex, normSlides: normSlides,
      TPL_DOCX: TPL_DOCX, XML_DECL_STD: XML_DECL_STD,
      /* 批次 F:图表 / 内嵌工作簿(方案 §12;验收脚本直呼 S28–S32 各层用) */
      xlsxMinimal: xlsxMinimal, chartXml: chartXml, chartGridOf: chartGridOf,
      normChartOf: normChartOf, colName: colName, numStr: numStr,
      chartPartOf: chartPartOf, embeddingPartOf: embeddingPartOf,
      nextChartIndex: nextChartIndex, nextEmbeddingIndex: nextEmbeddingIndex,
      chartRelsText: chartRelsText,
      pptxGraphicFrame: pptxGraphicFrame, nextShapeId: nextShapeId,
      docxChartInline: docxChartInline,
      checkChartChain: checkChartChain, checkEmbeddedBooks: checkEmbeddedBooks,
      checkGraphicHosts: checkGraphicHosts,
      CT_CHART: CT_CHART, REL_CHART: REL_CHART, REL_PACKAGE: REL_PACKAGE,
      URI_CHART: URI_CHART, URI_TABLE: URI_TABLE, URI_PICTURE: URI_PICTURE,
      SHEET_NAME: SHEET_NAME, PPTX_CHART_XF: PPTX_CHART_XF,
      CHART_AXIS_CAT: CHART_AXIS_CAT, CHART_AXIS_VAL: CHART_AXIS_VAL,
      /* 批次 G:页眉页脚(方案 §13;验收脚本直呼 S34 / S35 各层用) */
      headerPart: headerPart, footerPart: footerPart, hfPartOf: hfPartOf,
      nextHfIndex: nextHfIndex, relsEnsureTarget: relsEnsureTarget,
      docxHfApplyText: docxHfApplyText, sectRefOf: sectRefOf, sectRefTarget: sectRefTarget,
      insertByOrder: insertByOrder, orderIndexOf: orderIndexOf,
      DOCX_SECTPR_ORDER: DOCX_SECTPR_ORDER, DOCX_SETTINGS_ORDER: DOCX_SETTINGS_ORDER,
      hasSectChild: hasSectChild, docxSetHeaderFooter: docxSetHeaderFooter,
      pptxSetHeaderFooter: pptxSetHeaderFooter,
      pptxHfFlag: pptxHfFlag, pptxHasSldNumField: pptxHasSldNumField,
      pptxSlideHfApply: pptxSlideHfApply, pptxSlideHfShape: pptxSlideHfShape,
      pptxHfPhDef: pptxHfPhDef, pptxSlideLayoutOf: pptxSlideLayoutOf,
      pptxSpXfrmOf: pptxSpXfrmOf, pptxClearParas: pptxClearParas,
      pptxSldNumPara: pptxSldNumPara, SLIDENUM_GUID: SLIDENUM_GUID,
      CT_DOCX: CT_DOCX, REL_HEADER: REL_HEADER, REL_FOOTER: REL_FOOTER,
      REL_SETTINGS: REL_SETTINGS,
      /* 批次 H:表格 / 图片 / 套用版式(方案 §14;验收脚本直呼 S37–S39 各层用) */
      normTableOf: normTableOf, wTblOf: wTblOf, wTblPrOf: wTblPrOf, wTblWidths: wTblWidths,
      aTblOf: aTblOf, pptxTableGraphicFrame: pptxTableGraphicFrame,
      docxAddTable: docxAddTable, pptxAddTable: pptxAddTable, TABLE_STYLE_ID: TABLE_STYLE_ID,
      normImagesOf: normImagesOf, imgMagicOf: imgMagicOf, imgSizeOf: imgSizeOf,
      imgExtentOf: imgExtentOf, nextMediaIndex: nextMediaIndex, mediaPartOf: mediaPartOf,
      docxPicInline: docxPicInline, pptxPicShape: pptxPicShape,
      docxAddImage: docxAddImage, pptxAddImage: pptxAddImage,
      pptxApplyLayout: pptxApplyLayout, enclosingShapeId: enclosingShapeId,
      REL_IMAGE: REL_IMAGE, EMU_PER_PX: EMU_PER_PX, IMG_KIND: IMG_KIND,
      DOCX_TBL_W: DOCX_TBL_W, DOCX_IMG_MAX_CX: DOCX_IMG_MAX_CX, PPTX_IMG_MAX_CX: PPTX_IMG_MAX_CX,
      PPTX_TBL_ROW_H: PPTX_TBL_ROW_H
    }
  };

  var caps = missingCaps();
  if (caps.length) {
    api.available = false;
    api.why = "环境缺少: " + caps.join("、");
  } else {
    var why = payloadProbe();
    if (why) {
      api.available = false;
      api.why = why;
    } else {
      api.available = true;
      api.mode = "zip+xml+docx+pptx";
    }
  }
  W.OfficeWrite = api;
})(window);
