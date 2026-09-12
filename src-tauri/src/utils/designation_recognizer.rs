use regex::Regex;
use serde::{Deserialize, Serialize};

/// 识别方法枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RecognitionMethod {
    Regex,
    AI,
    Failed,
}

/// 番号语义标记：从文件名中识别并保留，不当作番号的一部分。
/// 番号归一（去标记得纯番号供刮削），标记单独保留（展示/筛选/版本维度）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignationMarkers {
    /// 多碟/分片标记（如 "A"、"B"、"CD1"），同片多文件 → 可关联
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    /// 中文字幕版（-C / -ch / 中文字幕）
    pub chinese_subtitle: bool,
    /// 无码破解（UC / 无修正 / 破解）
    pub uncensored: bool,
    /// 流出（LEAK / 流出 / 泄露）
    pub leaked: bool,
    /// VR 影片
    pub vr: bool,
    /// 分辨率/规格标记（4K / 8K / UHD）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    /// 文件名里带的无码厂牌词（carib / 1pon / 10mu / paco 等 D2Pass 系缩写，或 tokyo-hot），
    /// 值为 MetaTube provider 名。D2Pass 系同格式番号在各厂牌各有一部不同影片，厂牌是区分与
    /// 定向刮削的依据；Tokyo-Hot 的 `n0698` 格式本身不带厂牌字母，厂牌词是识别它的可靠线索。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub studio: Option<String>,
}

impl DesignationMarkers {
    /// 是否无任何标记
    pub fn is_empty(&self) -> bool {
        self.part.is_none()
            && !self.chinese_subtitle
            && !self.uncensored
            && !self.leaked
            && !self.vr
            && self.resolution.is_none()
            && self.studio.is_none()
    }

    /// 转为标签字符串集合（供复用 tags 入库/筛选）。
    pub fn to_tags(&self) -> Vec<String> {
        let mut tags = Vec::new();
        if self.chinese_subtitle {
            tags.push("中文字幕".to_string());
        }
        if self.uncensored {
            tags.push("无码破解".to_string());
        }
        if self.leaked {
            tags.push("流出".to_string());
        }
        if self.vr {
            tags.push("VR".to_string());
        }
        if let Some(res) = &self.resolution {
            tags.push(res.clone());
        }
        if let Some(part) = &self.part {
            tags.push(format!("分片{}", part));
        }
        tags
    }
}

/// 完整识别结果：纯番号 + 语义标记
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignationInfo {
    pub designation: String,
    pub markers: DesignationMarkers,
    /// 番号本身是否为无码作品（按格式/厂牌判定，区别于 markers.uncensored 的"有码作品无码流出版"）
    #[serde(default)]
    pub is_uncensored: bool,
}

/// 识别结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecognitionResult {
    pub success: bool,
    pub designation: Option<String>,
    #[serde(default)]
    pub markers: DesignationMarkers,
    /// 番号本身是否为无码作品（按格式/厂牌判定）
    #[serde(default)]
    pub is_uncensored: bool,
    pub method: RecognitionMethod,
    pub message: String,
}

/// AI 提供商配置
#[derive(Debug, Clone)]
pub struct AIProvider {
    pub provider: String,
    pub model: String,
    pub api_key: String,
    pub endpoint: Option<String>,
}

/// 番号识别器
///
/// 负责从视频文件名或标题中识别番号（JAV designation）
/// 支持多种常见格式的正则表达式匹配和 AI 识别
pub struct DesignationRecognizer {
    /// 正则表达式模式列表，每个模式包含正则表达式和优先级
    regex_patterns: Vec<(Regex, i32)>,
    /// AI 客户端（可选）
    ai_provider: Option<AIProvider>,
}

/// 番号识别正则（通用路径，按优先级），进程内只编译一次。文件名带厂牌缩写时
/// 优先走 [`STUDIO_RULES`] 的厂牌快速通道，这里只处理其余情况。
///
/// 多捕获组的模式：组 1 = 前缀，组 2 = 数字（番号归一为「前缀-数字」；纯数字前缀保留原分隔符）。
/// 单捕获组的模式：组 1 即完整番号（无分隔符格式，如 Tokyo-Hot 的 `n0698`）。
static REGEX_PATTERNS: std::sync::LazyLock<Vec<(Regex, i32)>> = std::sync::LazyLock::new(|| {
    vec![
        // FC2（最高）：PPV 标记可有可无（FC2-PPV-1234567 / FC2PPV1234567 / FC2-821825），
        // 归一为 FC2-数字。不带 PPV 时若落到通用规则会被截成伪番号 FC2-82182
        (Regex::new(r"(?i)(FC2)(?:[-_ ]?PPV)?[-_ ]?(\d{6,8})").unwrap(), 100),
        // XXX-AV（无码，前缀本身带连字符）：XXX-AV-20845。须高于标准格式，否则会被截成 AV-20845
        (Regex::new(r"(?i)(XXX-AV)-?(\d{4,5})").unwrap(), 96),
        // 素人：数字前缀 + 字母 + 连字符 + 数字（390JAC-132 / 300MAAN-783）。
        // 须高于标准格式，否则会被截成 "JAC-132"。左侧加非字母数字边界，
        // 避免把更长数字串（如日期戳 20231231SSIS-001）吞进前缀。
        (Regex::new(r"(?i)(?:^|[^0-9A-Z])(\d{2,4}[A-Z]{2,6})-(\d{3,5})").unwrap(), 95),
        // Kirari 系（AVE 无码）：序号带字母 S（MKBD-S94 / MKD-S94），标准规则不认
        (Regex::new(r"(?i)(?:^|[^A-Z0-9])(MKBD|MKD)-(S\d{2,3})(?:[^0-9]|$)").unwrap(), 92),
        // 标准带连字符 ABC-123
        (Regex::new(r"(?i)([A-Z]{2,6})-(\d{3,5})").unwrap(), 90),
        // 字母+数字混合前缀 T28-123 / KIN8-1675
        (Regex::new(r"(?i)([A-Z]+\d+)-(\d{3,5})").unwrap(), 85),
        // 无连字符 ABC123
        (Regex::new(r"(?i)([A-Z]{2,6})(\d{3,5})(?:[^A-Z0-9]|$)").unwrap(), 80),
        // Tokyo-Hot：单字母 n/k + 4 位数字（n0698 / k1234），无分隔符，整体即番号（单捕获组）。
        // 两侧加边界：不从更长的字母/数字串里截取（4k1080 / k12345 都不算）
        (Regex::new(r"(?i)(?:^|[^A-Z0-9])([NK]\d{4})(?:[^0-9]|$)").unwrap(), 75),
        // 纯数字 123456-789 / 123456_999（加勒比 / 一本道 / 帕高等 D2Pass 系无码）。
        // 分隔符是厂牌区分符，须保留（见 best_candidate）。左侧加非数字边界，
        // 避免把更长数字串（如时间戳 20231105_123）截出伪番号。
        (Regex::new(r"(?i)(?:^|[^0-9])(\d{6})[_-](\d{3,5})").unwrap(), 70),
        // 天然むすめ两位序号 123456_12（仅下划线；右侧加边界，避免截断三位序号）
        (Regex::new(r"(?i)(?:^|[^0-9])(\d{6})_(\d{2})(?:[^0-9]|$)").unwrap(), 69),
        // 空格分隔 ABC 123（最低；易误匹配标题词，仅作兜底）
        (Regex::new(r"(?i)([A-Z]{2,6})\s+(\d{3,5})").unwrap(), 60),
    ]
});

/// 已知 VR 番号前缀（命中即判 VR）
static VR_PREFIXES: &[&str] = &[
    "SIVR", "DSVR", "VRKM", "EXVR", "KMVR", "MDVR", "CRVR", "WPVR", "TMAVR", "DOVR", "AJVR",
    "MAXVR", "KAVR", "HUNVR", "SAVR", "TPVR", "FSVR", "CBIKMV", "VOVS",
];

/// VR 文件标记：vr 词、180/3D、mkx200 等投影标记
static VR_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-z])vr(?:[^a-z]|$)|_180_|_3dh|mkx-?200|lr_180").unwrap());
/// 分辨率/规格：4K / 8K / UHD
static RES_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])([48]k|uhd)(?:[^a-z0-9]|$)").unwrap());
/// 无码破解（英文 token）
static UC_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-z])(?:uncensored|uc)(?:[^a-z]|$)").unwrap());
/// 流出（英文 token）
static LEAK_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-z])(?:leaked|leak)(?:[^a-z]|$)").unwrap());
/// 多碟/分段标记 CD1 / DISC2 / PART003 / PT2 / VOL2（组 1=单位，组 2=序号）
static CD_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"(?i)^(cd|disc|part|pt|vol)0*(\d{1,3})$").unwrap());

/// 分段文件名解析：结尾数字型分段后缀（part/pt/cd/disc/vol/分卷/第N部）。
/// 分段标记前须为**数字**（分支 1，如 `724Part2`）或**分隔符**（分支 2，如 `-CD2`），
/// 否则会把番号前缀里的字母子串（如 `ABCD-123` 的 `CD`）误判为分段。
/// 两分支的基名分别落在捕获组 1 / 2，序号固定在组 3。
static STACK_NUM_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?i)^(?:(.*\d)[\s._-]*(?:part|pt|cd|disc|vol|分卷|第)|(.*?)[\s._-]+(?:part|pt|cd|disc|vol|分卷|第))[\s._-]*0*(\d{1,3})(?:部|集|話|话)?$").unwrap()
});

/// 分段文件名解析：D2Pass 系（加勒比 / 一本道 / 天然むすめ等）官方分段命名——画质词 + 段序号
/// （`012413-001-carib-fhd1` / `-whole_hd2` / `-high_3`）。画质词前须有分隔符；序号限 1-2 位，
/// 避免把 `-hd720` 这类分辨率当成第 720 段。基名在组 1（含厂牌词），序号在组 2。
static STACK_QUALITY_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?i)^(.*?)[\s._-]+(?:whole_hd|fhd|hd|sd|high|mid|low)[\s._-]?0*(\d{1,2})$").unwrap()
});

/// 分段文件名解析：结尾单字母 A/B/D（C 留给中文字幕，避免把字幕版误并为分段）。
/// 字母前须为数字，避免吃掉普通单词结尾。
static STACK_LETTER_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"(?i)^(.*\d)[\s._-]?([abd])$").unwrap());

/// 分段文件名解析：番号后接**裸数字**分段（无 part/cd 等单位后缀），如 `FC2-PPV-2458342-2`。
/// 基名（组 1）须**含连字符且以数字结尾**（形如完整番号，如 `FC2-PPV-2458342`），分段序号 1-3 位（组 2）。
/// 靠「基名必含连字符」这一约束，避免把普通番号（`ABC-123` 基名 `ABC` 无连字符）
/// 或纯数字番号（`123456-789` 基名 `123456` 无连字符）的数字段误判为分段。
static STACK_BARE_NUM_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?i)^(.*-\d+)[\s._-]0*(\d{1,3})(?:部|集|話|话)?$").unwrap()
});

/// 已知无码厂牌前缀（番号本身即无码作品）。纯数字前缀（加勒比/一本道/天然むすめ/帕高等）
/// 另行按"前缀全为数字"判定，Tokyo-Hot 的 `n0698` 格式按 [`TOKYOHOT_RE`] 判定，均不在此列。
/// 末行为 AVE 系无码 DVD 厂牌（S Model / Catwalk Poison / Sky Angel / Laforet / Red Hot Jam / Kirari）
/// 与前缀本身带连字符的 XXX-AV。
static UNCENSORED_PREFIXES: &[&str] = &[
    "FC2", "HEYZO", "KIN8", "MYWIFE", "CARIB", "CARIBBEANCOM", "CARIBBEANCOMPR", "PACO",
    "PACOPACOMAMA", "HEYDOUGA", "GACHINCO", "GACHI", "1PONDO", "10MU", "10MUSUME", "TOKYOHOT",
    "SMBD", "SMD", "S2M", "S2MBD", "CWP", "CWPBD", "SKY", "SKYHD", "LAF", "LAFBD", "RHJ", "MKBD", "MKD",
    "XXX-AV",
];

/// 前缀本身带连字符的厂牌（`XXX-AV-20845`）：归一番号有两个分隔符，校验时按整段前缀放行。
static MULTI_SEGMENT_PREFIXES: &[&str] = &["XXX-AV"];

/// 序号本身就是四位补零的厂牌（`HEYZO-0169` 即正式番号），[`canonicalize_designation`] 不去其前导零。
static ZERO_PADDED_PREFIXES: &[&str] = &["HEYZO", "KIN8"];

/// Tokyo-Hot 番号格式：单字母 n/k + 4 位数字（`n0698` / `k1234`），无分隔符
static TOKYOHOT_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"(?i)^[nk]\d{4}$").unwrap());

/// 判定番号本身是否为无码作品（按格式/厂牌）。
///
/// - 纯数字前缀（`010120-001` / `123456_789`：加勒比 / 一本道 / 天然むすめ / 帕高等）→ 无码
/// - Tokyo-Hot 格式（`n0698` / `k1234`）→ 无码
/// - 已知无码厂牌前缀（FC2 / HEYZO / KIN8 / MYWIFE / SMBD 等）→ 无码
/// - 素人数字+字母前缀（`300MIUM-700` / `390JAC-132`）含字母 → 有码，不误判
///
/// 注意：这判定"作品天生无码"，与 [`DesignationMarkers::uncensored`]（有码作品的无码破解流出版）
/// 是不同维度。入参应为归一后的纯番号（`PREFIX-NUMBER`）。
pub fn is_uncensored_designation(designation: &str) -> bool {
    let upper = designation.trim().to_uppercase();
    if TOKYOHOT_RE.is_match(&upper) {
        return true;
    }
    let prefix = upper.split(['-', '_']).next().unwrap_or("");
    if prefix.is_empty() {
        return false;
    }
    // 纯数字前缀 → 无码（加勒比/一本道等）
    if prefix.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    if UNCENSORED_PREFIXES.contains(&prefix) {
        return true;
    }
    // 前缀本身带连字符的厂牌（XXX-AV-20845）：取最后一个分隔符之前的整段比对
    upper
        .rsplit_once(['-', '_'])
        .is_some_and(|(full_prefix, _)| UNCENSORED_PREFIXES.contains(&full_prefix))
}

/// 把文件名式 / 口语式番号改写成各站收录的正式写法（厂牌命名怪癖），供搜索前归一：
/// - 字母前缀 + 四位以上且以 0 开头的序号 → 去前导零、补足三位（`BIBIVR-0169` → `BIBIVR-169`，
///   DMM 五位补零 `SIVR-00123` → `SIVR-123`）；HEYZO 等本身四位补零的厂牌与 FC2 不改
/// - Kirari 蓝光 `MKBD-094` / `MKBD-94` → `MKBD-S94`（该系列正式番号序号带 S、不补零；
///   `MKD-094` 与 `MKD-S94` 是两部不同影片，MKD 不改写）
///
/// D2Pass 纯数字番号（分隔符 / 前导零都有含义）与 Tokyo-Hot 格式原样返回（仅大写）。
pub fn canonicalize_designation(designation: &str) -> String {
    let upper = designation.trim().to_uppercase();
    if is_d2pass_designation(&upper) || TOKYOHOT_RE.is_match(&upper) {
        return upper;
    }
    let Some((prefix, number)) = upper.rsplit_once('-') else {
        return upper;
    };
    if prefix.is_empty() || !prefix.chars().any(|c| c.is_ascii_alphabetic()) {
        return upper;
    }
    if prefix == "MKBD" {
        let digits = number.trim_start_matches('S').trim_start_matches('0');
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            return format!("MKBD-S{}", digits);
        }
        return upper;
    }
    let is_digits = !number.is_empty() && number.chars().all(|c| c.is_ascii_digit());
    let keep_zeros = ZERO_PADDED_PREFIXES.contains(&prefix) || prefix.starts_with("FC2");
    if is_digits && number.len() >= 4 && number.starts_with('0') && !keep_zeros {
        return format!("{}-{:0>3}", prefix, number.trim_start_matches('0'));
    }
    upper
}

/// 厂牌识别规则：文件名带厂牌词时，按该厂牌自己的番号格式识别（厂牌快速通道）。
///
/// 收录两类：
/// - D2Pass 系无码厂牌：共用「MMDDYY-NNN」纯数字格式，同一日期序号在各厂牌各有一部不同影片
///   （加勒比 `110615-001` 与一本道 `110615_001` 不是同一部），分隔符（加勒比 `-`，其余 `_`）
///   和厂牌词是唯一区分依据。
/// - Tokyo-Hot：`n0698` / `k1234` 单字母 + 4 位数字，文件名常写作 `Tokyo-Hot n0698`。
///
/// 新增厂牌只需往 [`STUDIO_RULES`] 加一条。
struct StudioRule {
    /// 厂牌名，与 MetaTube provider 名一致，便于定向搜索
    name: &'static str,
    /// 文件名里的厂牌词（整词匹配，忽略大小写；可含分隔符，如 `tokyo-hot`）
    alias_re: Regex,
    /// 该厂牌番号格式：各捕获组按序用 `separator` 拼接即为归一番号
    code_re: Regex,
    /// 归一分隔符
    separator: &'static str,
}

/// 厂牌规则表（进程内只编译一次）。
static STUDIO_RULES: std::sync::LazyLock<Vec<StudioRule>> = std::sync::LazyLock::new(|| {
    // 厂牌词：两侧非字母数字边界的整词（`carib-110615-001` / `110615_001-1pon-1080p`）
    let words = |alts: &str| -> Regex {
        Regex::new(&format!(r"(?i)(?:^|[^a-z0-9])(?:{alts})(?:[^a-z0-9]|$)")).unwrap()
    };
    // D2Pass 系：6 位日期 + 分隔符 + 序号。分隔符接受 `-`/`_`（文件名可能写错，归一时按厂牌改写），
    // 两侧加非数字边界，避免截断更长数字串。
    let d2pass = |digits: usize| -> Regex {
        Regex::new(&format!(r"(?:^|[^0-9])(\d{{6}})[-_](\d{{{digits}}})(?:[^0-9]|$)")).unwrap()
    };
    vec![
        StudioRule {
            name: "Caribbeancom",
            alias_re: words("carib|caribbean|caribbeancom"),
            code_re: d2pass(3),
            separator: "-",
        },
        StudioRule {
            name: "CaribbeancomPR",
            alias_re: words("caribpr|caribbeancompr"),
            code_re: d2pass(3),
            separator: "_",
        },
        StudioRule {
            name: "1Pondo",
            alias_re: words("1pon|1pondo"),
            code_re: d2pass(3),
            separator: "_",
        },
        StudioRule {
            name: "10musume",
            alias_re: words("10mu|10musume"),
            code_re: d2pass(2),
            separator: "_",
        },
        StudioRule {
            name: "PACOPACOMAMA",
            alias_re: words("paco|pacopacomama"),
            code_re: d2pass(3),
            separator: "_",
        },
        StudioRule {
            name: "MURAMURA",
            alias_re: words("mura|muramura"),
            code_re: d2pass(3),
            separator: "_",
        },
        StudioRule {
            name: "TOKYO-HOT",
            alias_re: words(r"tokyo[\s._-]?hot"),
            // 单捕获组：整体即番号，无分隔符
            code_re: Regex::new(r"(?i)(?:^|[^a-z0-9])([nk]\d{4})(?:[^0-9]|$)").unwrap(),
            separator: "",
        },
    ]
});

/// D2Pass 系纯数字番号格式：`MMDDYY-NNN` / `MMDDYY_NNN` / `MMDDYY_NN`
static D2PASS_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"^\d{6}[-_]\d{2,3}$").unwrap());

/// 从文件名中找厂牌词（整词匹配，忽略大小写），返回命中的规则。
fn detect_studio_rule(title: &str) -> Option<&'static StudioRule> {
    STUDIO_RULES.iter().find(|rule| rule.alias_re.is_match(title))
}

/// 去掉文件名里的厂牌词（carib / 1pon / tokyo-hot 等，替换为空格）。
/// 厂牌词是番号的伴随信息而非番号本身，比对「输入是否只是番号加噪声」前先剔除。
pub fn strip_studio_tags(title: &str) -> String {
    STUDIO_RULES.iter().fold(title.to_string(), |acc, rule| {
        rule.alias_re.replace_all(&acc, " ").into_owned()
    })
}

/// 是否为 D2Pass 系纯数字番号（分隔符是厂牌区分符，不可改写）。
pub fn is_d2pass_designation(designation: &str) -> bool {
    D2PASS_RE.is_match(designation.trim())
}

/// 两个番号是否指同一作品：忽略大小写与非字母数字字符；
/// 但 D2Pass 系纯数字番号还要求分隔符一致（`110615-001` 与 `110615_001` 是不同影片）。
pub fn same_designation(a: &str, b: &str) -> bool {
    let canon = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .flat_map(|c| c.to_uppercase())
            .collect()
    };
    if canon(a) != canon(b) {
        return false;
    }
    let (a, b) = (a.trim(), b.trim());
    if is_d2pass_designation(a) && is_d2pass_designation(b) {
        return a.contains('_') == b.contains('_');
    }
    true
}

/// 解析分段/分卷文件名，返回 `(去后缀基名<大写归一>, 分段序号)`。
///
/// 用于同目录多文件归并（stacking）：`SSIS-724Part002` → `("SSIS-724", 2)`。
/// 识别四类分段：明确分段后缀（part/pt/cd/disc/vol/分卷/第N部）、D2Pass 官方的画质词+序号
/// （`012413-001-carib-fhd2` → `("012413-001-CARIB", 2)`）、结尾单字母 A/B/D、
/// 以及番号后接裸数字（`FC2-PPV-2458342-2` → `("FC2-PPV-2458342", 2)`，基名须含连字符防误拆）；
/// **不认中文字幕标记 C**，避免把「原版 + 中文字幕版」误并成分段。
/// 入参为不含扩展名的文件名（file stem）。基名为空或无后缀时返回 `None`。
pub fn parse_stack_part(file_stem: &str) -> Option<(String, i64)> {
    let s = file_stem.trim();

    let strip_base = |base: &str| -> String {
        base.trim_end_matches(|c: char| matches!(c, ' ' | '.' | '_' | '-'))
            .trim()
            .to_uppercase()
    };

    if let Some(cap) = STACK_NUM_RE.captures(s) {
        let base_raw = cap.get(1).or_else(|| cap.get(2)).map_or("", |m| m.as_str());
        let base = strip_base(base_raw);
        if let Ok(idx) = cap[3].parse::<i64>() {
            if !base.is_empty() && idx >= 1 {
                return Some((base, idx));
            }
        }
    }

    if let Some(cap) = STACK_QUALITY_RE.captures(s) {
        let base = strip_base(&cap[1]);
        if let Ok(idx) = cap[2].parse::<i64>() {
            if !base.is_empty() && idx >= 1 {
                return Some((base, idx));
            }
        }
    }

    if let Some(cap) = STACK_LETTER_RE.captures(s) {
        let base = strip_base(&cap[1]);
        let idx = match cap[2].to_ascii_uppercase().as_str() {
            "A" => 1,
            "B" => 2,
            "D" => 4,
            _ => return None,
        };
        if !base.is_empty() {
            return Some((base, idx));
        }
    }

    // 番号后接裸数字分段（无单位词），如 FC2-PPV-2458342-2 → ("FC2-PPV-2458342", 2)。
    // 放在最后作兜底：显式单位词/字母后缀优先，裸数字歧义最大，仅在基名含连字符时才认。
    if let Some(cap) = STACK_BARE_NUM_RE.captures(s) {
        let base = strip_base(&cap[1]);
        if let Ok(idx) = cap[2].parse::<i64>() {
            if !base.is_empty() && idx >= 1 {
                return Some((base, idx));
            }
        }
    }

    None
}

/// 目录名是否属于某分段组：就是组基名（`SIVR-015`）或组内某一段的名字（`SIVR-015-2`）。
/// `stack_key` 为 [`parse_stack_part`] 产出的大写基名。
pub fn dir_belongs_to_stack(dir_name: &str, stack_key: &str) -> bool {
    dir_name.eq_ignore_ascii_case(stack_key)
        || parse_stack_part(dir_name).is_some_and(|(base, _)| base.eq_ignore_ascii_case(stack_key))
}

/// 分段组的归并范围目录：同组各段在此目录下折叠成一张卡。
///
/// 通常就是视频所在目录；但各段可能被「归入同名目录」分别装进了以段名命名的子目录
/// （`SIVR-015-1/SIVR-015-1.mp4` + `SIVR-015-1/SIVR-015-2/SIVR-015-2.mp4`，或平级的
/// `SIVR-015-1/`、`SIVR-015-2/`），按直接父目录分组永远凑不齐。故从所在目录向上回溯，
/// 跳过名字属于该组（基名或某一段，见 [`dir_belongs_to_stack`]）的目录，直到遇到与该组
/// 无关的目录为止。入参 `stack_key` 为 [`parse_stack_part`] 产出的大写基名。
pub fn stack_scope_dir(dir_path: &str, stack_key: &str) -> String {
    let mut dir = std::path::Path::new(dir_path);
    while let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
        if !dir_belongs_to_stack(name, stack_key) {
            break;
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    dir.to_string_lossy().to_string()
}

/// 提取语义标记。
///
/// - 全局标记（VR / 4K / 无码 / 流出 / 中文字幕）：扫描整个文件名。
/// - 位置标记（分片 / 字幕后缀）：仅看番号之后紧邻的前 3 个 token，保守避免误判。
fn extract_markers(title: &str, suffix_start: Option<usize>, designation: &str) -> DesignationMarkers {
    let mut m = DesignationMarkers::default();
    let lower = title.to_lowercase();

    // ===== 全局标记 =====
    if VR_RE.is_match(&lower) {
        m.vr = true;
    }
    if let Some(cap) = RES_RE.captures(&lower) {
        m.resolution = Some(cap[1].to_uppercase());
    }
    if UC_RE.is_match(&lower) || lower.contains("无修正") || lower.contains("无码破解") || lower.contains("破解") {
        m.uncensored = true;
    }
    if LEAK_RE.is_match(&lower) || lower.contains("流出") || lower.contains("泄露") {
        m.leaked = true;
    }
    if lower.contains("中文字幕") || lower.contains("中字") {
        m.chinese_subtitle = true;
    }

    // VR：番号前缀属于已知 VR 厂牌
    if let Some(prefix) = designation.split('-').next() {
        if VR_PREFIXES.contains(&prefix.to_uppercase().as_str()) {
            m.vr = true;
        }
    }

    // ===== 位置标记（番号紧邻后缀）=====
    if let Some(start) = suffix_start {
        if start <= title.len() {
            let suffix = &title[start..];
            // 只看番号紧邻的连续片段（首个空白之前），避免把空格后的描述词/冠词（如 "a movie"
            // 的 "a"）误判为分片。VR/4K/无码 等全局标记仍扫全名，不受此限制。
            let region = suffix.split(char::is_whitespace).next().unwrap_or(suffix);
            for token in region
                .split(|c: char| matches!(c, '-' | '_' | '.' | '[' | ']' | '(' | ')'))
                .filter(|t| !t.is_empty())
                .take(3)
            {
                let tl = token.to_lowercase();
                match tl.as_str() {
                    "c" | "ch" | "chinese" => m.chinese_subtitle = true,
                    "a" | "b" | "d" => {
                        if m.part.is_none() {
                            m.part = Some(token.to_uppercase());
                        }
                    }
                    "uc" => m.uncensored = true,
                    "leak" | "leaked" => m.leaked = true,
                    _ => {
                        if let Some(cap) = CD_RE.captures(&tl) {
                            m.part = Some(format!("{}{}", cap[1].to_uppercase(), &cap[2]));
                        }
                    }
                }
            }
        }
    }

    m
}

/// 正则识别出的最佳候选
struct Candidate {
    /// 归一后的大写番号
    designation: String,
    /// 番号数字末尾在原串中的字节位置，用于界定「番号之后的后缀」以提取分片/字幕标记
    end: usize,
    /// 命中的厂牌（MetaTube provider 名），仅厂牌快速通道有值
    studio: Option<&'static str>,
}

impl DesignationRecognizer {
    /// 创建新的番号识别器实例
    pub fn new() -> Self {
        DesignationRecognizer {
            // 复用进程内只编译一次的正则（Regex 内部 Arc，clone 廉价）
            regex_patterns: REGEX_PATTERNS.clone(),
            ai_provider: None,
        }
    }

    /// 创建带 AI 提供商的识别器实例
    pub fn with_ai_provider(ai_provider: AIProvider) -> Self {
        let mut recognizer = Self::new();
        recognizer.ai_provider = Some(ai_provider);
        recognizer
    }

    /// 检查是否配置了 AI 提供商
    pub fn has_ai_provider(&self) -> bool {
        self.ai_provider.is_some()
    }

    /// 选出最佳番号候选。
    ///
    /// 1. 厂牌快速通道：文件名带已知厂牌缩写且该厂牌番号格式命中，直接采用（不与通用候选竞争，
    ///    否则 `carib-110615-001` 会被标准规则截成伪番号 `CARIB-11061`）。
    ///    缩写存在但格式不符（如 `SSIS-001 paco` 里的普通词）则丢弃厂牌，走通用路径。
    /// 2. 通用正则：按优先级取最佳；纯数字番号保留原始分隔符（`_` / `-` 是 D2Pass 厂牌区分符）。
    fn best_candidate(&self, title: &str) -> Option<Candidate> {
        if let Some(rule) = detect_studio_rule(title) {
            if let Some(cap) = rule.code_re.captures(title) {
                let groups: Vec<_> = (1..cap.len()).filter_map(|i| cap.get(i)).collect();
                if let Some(last) = groups.last() {
                    let designation = groups
                        .iter()
                        .map(|m| m.as_str())
                        .collect::<Vec<_>>()
                        .join(rule.separator);
                    return Some(Candidate {
                        designation: designation.to_uppercase(),
                        end: last.end(),
                        studio: Some(rule.name),
                    });
                }
            }
        }

        // (番号, 优先级, 起始位置, 数字末尾位置)
        let mut candidates: Vec<(String, i32, usize, usize)> = Vec::new();

        for (pattern, priority) in &self.regex_patterns {
            for captures in pattern.captures_iter(title) {
                let Some(whole) = captures.get(0) else {
                    continue;
                };
                let designation = match (captures.get(1), captures.get(2)) {
                    (Some(prefix), Some(number)) => {
                        // 纯数字番号（D2Pass 系）：分隔符是厂牌区分符，原样保留；其余归一为连字符
                        let keep_underscore = prefix.as_str().chars().all(|c| c.is_ascii_digit())
                            && &title[prefix.end()..number.start()] == "_";
                        let separator = if keep_underscore { "_" } else { "-" };
                        format!("{}{}{}", prefix.as_str(), separator, number.as_str())
                    }
                    // 单捕获组：组 1 即完整番号（无分隔符格式，如 Tokyo-Hot n0698）
                    (Some(code), None) => code.as_str().to_string(),
                    _ => captures[0].to_string(),
                };
                // 番号「之后」从最后一个捕获组末尾算起（无捕获组则用整体匹配末尾）
                let end = captures
                    .get(2)
                    .or_else(|| captures.get(1))
                    .map(|m| m.end())
                    .unwrap_or_else(|| whole.end());
                candidates.push((designation, *priority, whole.start(), end));
            }
        }

        // 优先级高的优先；同优先级位置靠后的优先
        candidates.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
        candidates.retain(|(designation, _, _, _)| self.is_valid_designation(designation));

        candidates
            .into_iter()
            .next()
            .map(|(designation, _, _, end)| Candidate {
                designation: designation.to_uppercase(),
                end,
                studio: None,
            })
    }

    /// 使用正则识别番号（返回归一后的纯番号，大写；含厂牌命名怪癖改写，见 [`canonicalize_designation`]）。
    pub fn recognize_with_regex(&self, title: &str) -> Option<String> {
        self.recognize_detailed(title).map(|c| c.designation)
    }

    /// 使用正则识别番号 + 语义标记（纯番号 + 分片/字幕/版本/VR/厂牌）。
    /// 番号统一改写成各站收录的正式写法（`bibivr-0169` → `BIBIVR-169`、`MKBD-094` → `MKBD-S94`）。
    pub fn recognize_detailed(&self, title: &str) -> Option<DesignationInfo> {
        let mut info = self.recognize_detailed_raw(title)?;
        info.designation = canonicalize_designation(&info.designation);
        Some(info)
    }

    /// 同 [`recognize_detailed`](Self::recognize_detailed)，但番号保留文件名里的原始写法（不做厂牌怪癖改写），
    /// 供需要与原输入逐字比对的场景（如判断输入是否只是番号加噪声）。
    pub fn recognize_detailed_raw(&self, title: &str) -> Option<DesignationInfo> {
        let cand = self.best_candidate(title)?;
        let mut markers = extract_markers(title, Some(cand.end), &cand.designation);
        markers.studio = cand.studio.map(str::to_string);
        let is_uncensored = is_uncensored_designation(&cand.designation);
        Some(DesignationInfo { designation: cand.designation, markers, is_uncensored })
    }

    /// 验证番号是否合理
    ///
    /// 1. Tokyo-Hot 格式（`n0698` / `k1234`）整体即番号，直接放行
    /// 2. 恰有一个分隔符（前缀本身带连字符的 XXX-AV 例外），前缀长度 2-8（含素人数字+字母前缀，如 300MAAN）
    /// 3. 数字部分 3-8 位（含 FC2 长数字；纯数字日期前缀允许天然むすめ的 2 位序号）
    /// 4. 排除常见非番号数字（分辨率等）
    fn is_valid_designation(&self, designation: &str) -> bool {
        if TOKYOHOT_RE.is_match(designation.trim()) {
            return true;
        }
        // D2Pass 系纯数字番号以 `_` 分隔（一本道等），与 `-` 同为合法分隔符
        let Some((prefix_part, number_part)) = designation.rsplit_once(['-', '_']) else {
            return false;
        };
        // 多段串（FC2-PPV-123 等）不算归一番号；仅已知的带连字符前缀（XXX-AV-20845）放行
        if prefix_part.contains(['-', '_'])
            && !MULTI_SEGMENT_PREFIXES.contains(&prefix_part.to_uppercase().as_str())
        {
            return false;
        }

        // 前缀长度 2-8（素人 300MAAN 等数字+字母前缀可达 7）
        let prefix_len = prefix_part.chars().count();
        if prefix_len < 2 || prefix_len > 8 {
            return false;
        }

        // 数字部分长度 3-8（支持 FC2 的 6-8 位）；纯数字日期前缀（D2Pass 系）允许 2 位序号
        let min_number_len = if prefix_part.chars().all(|c| c.is_ascii_digit()) { 2 } else { 3 };
        let number_len = number_part.chars().count();
        if number_len < min_number_len || number_len > 8 {
            return false;
        }

        // 排除常见非番号数字（分辨率等）
        if ["800", "1080", "720", "480", "360", "1440", "2160"].contains(&number_part) {
            return false;
        }

        true
    }

    /// 使用 AI 识别番号
    pub async fn recognize_with_ai(&self, title: &str) -> Result<String, String> {
        let provider = self.ai_provider.as_ref()
            .ok_or_else(|| "No AI provider configured".to_string())?;

        let client = crate::utils::proxy::apply_proxy_auto(
            wreq::Client::builder()
                .timeout(std::time::Duration::from_secs(15)),
        )
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?;

        // 构建提示词
        let prompt = format!(
            r#"请从以下视频文件名中识别出JAV番号（日本成人影片的编号）。

文件名: {}

JAV番号的常见格式包括：
- ABC-123 (字母-数字)
- ABC123 (字母数字)
- FC2-PPV-123456 / FC2-123456
- 123456-789 (加勒比)
- 123456_789 (一本道/帕高等，下划线不要改成连字符)
- 390JAC-132 (素人，数字+字母前缀)
- n0698 / k1234 (Tokyo-Hot，单字母+4位数字，无分隔符)
- XXX-AV-20845 (前缀本身带连字符，整体保留)

请只返回识别出的番号，去掉画质/字幕/分片等后缀（如 -C、-CD1、4K）。如果无法识别，请回复"未找到"。"#,
            title
        );

        // 根据provider类型发送不同的请求
        let default_endpoint = match provider.provider.as_str() {
            "openai" => "https://api.openai.com/v1".to_string(),
            "deepseek" => "https://api.deepseek.com/v1".to_string(),
            "claude" => "https://api.anthropic.com/v1".to_string(),
            _ => return Err("Unsupported AI provider".to_string()),
        };

        let base_url = provider.endpoint.as_ref().unwrap_or(&default_endpoint);

        if provider.provider == "claude" {
            self.call_claude_api(&client, base_url, &provider.api_key, &provider.model, &prompt).await
        } else {
            self.call_openai_compatible_api(&client, base_url, &provider.api_key, &provider.model, &prompt).await
        }
    }

    /// 调用 Claude API
    async fn call_claude_api(
        &self,
        client: &wreq::Client,
        base_url: &str,
        api_key: &str,
        model: &str,
        prompt: &str,
    ) -> Result<String, String> {
        let endpoint = format!("{}/messages", base_url.trim_end_matches('/'));

        let payload = serde_json::json!({
            "model": model,
            "max_tokens": 50,
            "messages": [{
                "role": "user",
                "content": prompt
            }]
        });

        let response = client
            .post(&endpoint)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("Claude API request failed: {}", e))?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(format!("Claude API error: {}", error_text));
        }

        let result: serde_json::Value = response.json().await
            .map_err(|e| format!("Failed to parse Claude response: {}", e))?;

        if let Some(content) = result["content"][0]["text"].as_str() {
            return self.normalize_ai_designation(content);
        }

        Err("Invalid Claude API response format".to_string())
    }

    /// 调用 OpenAI 兼容 API
    async fn call_openai_compatible_api(
        &self,
        client: &wreq::Client,
        base_url: &str,
        api_key: &str,
        model: &str,
        prompt: &str,
    ) -> Result<String, String> {
        let endpoint = format!("{}/chat/completions", base_url.trim_end_matches('/'));

        let payload = serde_json::json!({
            "model": model,
            "messages": [{
                "role": "user",
                "content": prompt
            }],
            "max_tokens": 50,
            "temperature": 0.3
        });

        let response = client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("OpenAI API request failed: {}", e))?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(format!("OpenAI API error: {}", error_text));
        }

        let result: serde_json::Value = response.json().await
            .map_err(|e| format!("Failed to parse OpenAI response: {}", e))?;

        if let Some(content) = result["choices"][0]["message"]["content"].as_str() {
            return self.normalize_ai_designation(content);
        }

        Err("Invalid OpenAI API response format".to_string())
    }

    /// 校验并规范化 AI 返回的番号
    fn normalize_ai_designation(&self, content: &str) -> Result<String, String> {
        let designation = content.trim();

        if designation.to_lowercase().contains("未找到")
            || designation.to_lowercase().contains("not found")
        {
            return Err("AI could not identify designation".to_string());
        }

        // 直接校验 AI 返回的整段内容
        let upper = designation.to_uppercase();
        if self.is_valid_designation(&upper) {
            return Ok(canonicalize_designation(&upper));
        }

        // 校验不通过，尝试用正则从 AI 回复里再抽取一次番号
        if let Some(extracted) = self.recognize_with_regex(designation) {
            return Ok(extracted);
        }

        Err("AI could not identify designation".to_string())
    }

    /// 组合识别方法（先正则后 AI），结果含语义标记。
    ///
    /// 标记从原始文件名中提取：正则路径用番号位置精确取后缀标记；
    /// AI 路径仅取全局标记（VR/4K/无码/流出/中文字幕）。
    pub async fn recognize(&self, title: &str, force_ai: bool) -> Result<RecognitionResult, String> {
        // 1. 如果不强制使用 AI，先尝试正则表达式识别
        if !force_ai {
            if let Some(info) = self.recognize_detailed(title) {
                return Ok(RecognitionResult {
                    success: true,
                    designation: Some(info.designation),
                    markers: info.markers,
                    is_uncensored: info.is_uncensored,
                    method: RecognitionMethod::Regex,
                    message: "识别成功（正则匹配）".to_string(),
                });
            }
        }

        // 2. 如果正则识别失败或强制使用 AI，尝试 AI 识别
        if self.ai_provider.is_some() {
            match self.recognize_with_ai(title).await {
                Ok(designation) => {
                    let markers = extract_markers(title, None, &designation);
                    let is_uncensored = is_uncensored_designation(&designation);
                    return Ok(RecognitionResult {
                        success: true,
                        designation: Some(designation),
                        markers,
                        is_uncensored,
                        method: RecognitionMethod::AI,
                        message: "识别成功（AI）".to_string(),
                    });
                }
                Err(e) => {
                    log::warn!(
                        "[designation] event=ai_recognition_failed title={} error={}",
                        title,
                        e
                    );
                }
            }
        }

        // 3. 所有方法都失败
        Ok(RecognitionResult {
            success: false,
            designation: None,
            markers: DesignationMarkers::default(),
            is_uncensored: false,
            method: RecognitionMethod::Failed,
            message: "无法识别番号".to_string(),
        })
    }
}

impl Default for DesignationRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============ 覆盖面：基础格式 ============

    #[test]
    fn recognizes_standard_and_no_hyphen() {
        let r = DesignationRecognizer::new();
        assert_eq!(r.recognize_with_regex("ABC-123.mp4"), Some("ABC-123".into()));
        assert_eq!(r.recognize_with_regex("[JAV] ABC-123 [1080p].mp4"), Some("ABC-123".into()));
        assert_eq!(r.recognize_with_regex("ABC123.mp4"), Some("ABC-123".into()));
        assert_eq!(r.recognize_with_regex("SSIS-456.mp4"), Some("SSIS-456".into()));
        assert_eq!(r.recognize_with_regex("T28-123.mp4"), Some("T28-123".into()));
    }

    #[test]
    fn recognizes_fc2() {
        let r = DesignationRecognizer::new();
        assert_eq!(r.recognize_with_regex("FC2-PPV-1234567.mp4"), Some("FC2-1234567".into()));
        assert_eq!(r.recognize_with_regex("FC2PPV1234567.mp4"), Some("FC2-1234567".into()));
        // 不带 PPV 的写法：不得截成伪番号 FC2-82182
        assert_eq!(r.recognize_with_regex("FC2-821825.mp4"), Some("FC2-821825".into()));
        assert_eq!(r.recognize_with_regex("FC2_PPV_821825.mp4"), Some("FC2-821825".into()));
        assert_eq!(r.recognize_with_regex("fc2-ppv 821825.mp4"), Some("FC2-821825".into()));
    }

    // ============ Tokyo-Hot ============

    #[test]
    fn recognizes_tokyohot_with_brand_word() {
        let r = DesignationRecognizer::new();
        // 用户实际文件名：厂牌词 + 单字母 4 位数字，整体即番号（无分隔符）
        let info = r.recognize_detailed("Tokyo-Hot n0698.wmv").unwrap();
        assert_eq!(info.designation, "N0698");
        assert_eq!(info.markers.studio.as_deref(), Some("TOKYO-HOT"));
        assert!(info.is_uncensored);
        assert_eq!(r.recognize_with_regex("tokyohot-k1234.mp4"), Some("K1234".into()));
        assert_eq!(r.recognize_with_regex("tokyo_hot_n0698_1080p.mp4"), Some("N0698".into()));
    }

    #[test]
    fn recognizes_tokyohot_bare_code() {
        let r = DesignationRecognizer::new();
        // 无厂牌词：走通用规则，同样整体即番号，不带厂牌线索
        let info = r.recognize_detailed("n0698.mp4").unwrap();
        assert_eq!(info.designation, "N0698");
        assert!(info.markers.studio.is_none());
        assert!(info.is_uncensored);
        // 标准番号优先级更高，不被 Tokyo-Hot 规则抢走
        assert_eq!(r.recognize_with_regex("SSIS-001 k1234.mp4"), Some("SSIS-001".into()));
        // 不从更长的字母/数字串里截取
        assert_eq!(r.recognize_with_regex("k12345.mp4"), None);
        assert_eq!(r.recognize_with_regex("4k1080.mp4"), None);
    }

    #[test]
    fn strip_studio_tags_removes_brand_words() {
        assert_eq!(strip_studio_tags("carib-110615-001").trim(), "110615-001");
        assert_eq!(strip_studio_tags("Tokyo-Hot n0698").trim(), "n0698");
        // 普通番号不受影响
        assert_eq!(strip_studio_tags("SSIS-001"), "SSIS-001");
    }

    // ============ 前缀带连字符 / 序号带字母的厂牌 ============

    #[test]
    fn recognizes_xxx_av_with_hyphenated_prefix() {
        let r = DesignationRecognizer::new();
        // 用户实际文件名：不得截成 AV-20845
        let info = r.recognize_detailed("xxx-av-20845.mp4").unwrap();
        assert_eq!(info.designation, "XXX-AV-20845");
        assert!(info.is_uncensored);
        assert_eq!(r.recognize_with_regex("XXX-AV-20845-C.mp4"), Some("XXX-AV-20845".into()));
        assert_eq!(r.recognize_with_regex("[XXX-AV] xxx-av20845 1080p.mp4"), Some("XXX-AV-20845".into()));
    }

    #[test]
    fn recognizes_kirari_letter_number() {
        let r = DesignationRecognizer::new();
        // Kirari 系正式番号序号带 S（刮削改名后的文件名）
        let info = r.recognize_detailed("MKBD-S94.mkv").unwrap();
        assert_eq!(info.designation, "MKBD-S94");
        assert!(info.is_uncensored);
        assert_eq!(r.recognize_with_regex("mkd-s143 kirari.mp4"), Some("MKD-S143".into()));
        // 用户原始文件名：识别后按厂牌怪癖改写成正式番号
        assert_eq!(r.recognize_with_regex("MKBD-094.mkv"), Some("MKBD-S94".into()));
        assert_eq!(r.recognize_with_regex("bibivr-0169.mp4"), Some("BIBIVR-169".into()));
        // 分片标记仍按原位置提取
        let info = r.recognize_detailed("bibivr-0169-C.mp4").unwrap();
        assert_eq!(info.designation, "BIBIVR-169");
        assert!(info.markers.chinese_subtitle);
    }

    #[test]
    fn canonicalize_designation_follows_label_quirks() {
        // 四位补零去零补足三位（用户实际文件名 bibivr-0169，正式番号 BIBIVR-169）
        assert_eq!(canonicalize_designation("bibivr-0169"), "BIBIVR-169");
        assert_eq!(canonicalize_designation("SIVR-00123"), "SIVR-123");
        assert_eq!(canonicalize_designation("IPX-0001"), "IPX-001");
        // 本身四位补零 / 无前导零 / 三位的都不动
        assert_eq!(canonicalize_designation("HEYZO-0169"), "HEYZO-0169");
        assert_eq!(canonicalize_designation("KIN8-1675"), "KIN8-1675");
        assert_eq!(canonicalize_designation("SSIS-001"), "SSIS-001");
        assert_eq!(canonicalize_designation("DASD-1004"), "DASD-1004");
        assert_eq!(canonicalize_designation("FC2-PPV-0123456"), "FC2-PPV-0123456");
        // Kirari 蓝光：MKBD-094 / MKBD-94 / MKBD-S094 → MKBD-S94；MKD 不改写
        assert_eq!(canonicalize_designation("MKBD-094"), "MKBD-S94");
        assert_eq!(canonicalize_designation("mkbd-94"), "MKBD-S94");
        assert_eq!(canonicalize_designation("MKBD-S094"), "MKBD-S94");
        assert_eq!(canonicalize_designation("MKBD-S94"), "MKBD-S94");
        assert_eq!(canonicalize_designation("MKD-094"), "MKD-094");
        // D2Pass / Tokyo-Hot / 前缀带连字符：原样
        assert_eq!(canonicalize_designation("010120-001"), "010120-001");
        assert_eq!(canonicalize_designation("110615_01"), "110615_01");
        assert_eq!(canonicalize_designation("n0698"), "N0698");
        assert_eq!(canonicalize_designation("XXX-AV-20845"), "XXX-AV-20845");
        assert_eq!(canonicalize_designation("390JAC-0132"), "390JAC-132");
    }

    // ============ 覆盖面：新增格式 ============

    #[test]
    fn recognizes_amateur_digit_prefix() {
        let r = DesignationRecognizer::new();
        // 素人：数字前缀不能被截掉
        assert_eq!(r.recognize_with_regex("390JAC-132.mp4"), Some("390JAC-132".into()));
        assert_eq!(r.recognize_with_regex("300MAAN-783.mp4"), Some("300MAAN-783".into()));
    }

    #[test]
    fn recognizes_uncensored_underscore_and_brands() {
        let r = DesignationRecognizer::new();
        // 下划线是一本道等厂牌的分隔符，须原样保留（与加勒比的连字符区分）
        assert_eq!(r.recognize_with_regex("123456_999.mp4"), Some("123456_999".into()));
        assert_eq!(r.recognize_with_regex("123456-999.mp4"), Some("123456-999".into()));
        assert_eq!(r.recognize_with_regex("HEYZO-1234.mp4"), Some("HEYZO-1234".into()));
        assert_eq!(r.recognize_with_regex("KIN8-1675.mp4"), Some("KIN8-1675".into()));
        assert_eq!(r.recognize_with_regex("MYWIFE-1394.mp4"), Some("MYWIFE-1394".into()));
    }

    #[test]
    fn recognizes_space_separator() {
        let r = DesignationRecognizer::new();
        assert_eq!(r.recognize_with_regex("ABC 123.mp4"), Some("ABC-123".into()));
    }

    #[test]
    fn classifies_uncensored_designation() {
        // 无码：纯数字前缀（加勒比/一本道等）+ 已知无码厂牌
        assert!(is_uncensored_designation("123456-999"));
        assert!(is_uncensored_designation("010120-001"));
        assert!(is_uncensored_designation("110615_001"));
        assert!(is_uncensored_designation("110615_01"));
        assert!(is_uncensored_designation("FC2-1234567"));
        assert!(is_uncensored_designation("HEYZO-1234"));
        assert!(is_uncensored_designation("KIN8-1675"));
        assert!(is_uncensored_designation("heyzo-1234")); // 大小写不敏感
        // AVE 系无码 DVD 厂牌 / Tokyo-Hot 格式 / 前缀带连字符的 XXX-AV
        assert!(is_uncensored_designation("SMBD-050"));
        assert!(is_uncensored_designation("MKBD-S94"));
        assert!(is_uncensored_designation("N0698"));
        assert!(is_uncensored_designation("k1234"));
        assert!(is_uncensored_designation("XXX-AV-20845"));
        // 有码：标准番号 + 素人（数字+字母前缀，含字母不误判）
        assert!(!is_uncensored_designation("SSIS-001"));
        assert!(!is_uncensored_designation("300MIUM-700"));
        assert!(!is_uncensored_designation("390JAC-132"));
        assert!(!is_uncensored_designation(""));
    }

    #[test]
    fn recognize_detailed_sets_is_uncensored() {
        let r = DesignationRecognizer::new();
        assert!(r.recognize_detailed("FC2-PPV-1234567.mp4").unwrap().is_uncensored);
        assert!(r.recognize_detailed("123456_999.mp4").unwrap().is_uncensored);
        assert!(!r.recognize_detailed("SSIS-001.mp4").unwrap().is_uncensored);
    }

    #[test]
    fn suffix_letter_does_not_break_designation() {
        let r = DesignationRecognizer::new();
        // 后缀字母不并入番号
        assert_eq!(r.recognize_with_regex("SSIS-001A.mp4"), Some("SSIS-001".into()));
    }

    #[test]
    fn filters_resolution_numbers() {
        let r = DesignationRecognizer::new();
        assert_eq!(r.recognize_with_regex("ABC-1080.mp4"), None);
        assert_eq!(r.recognize_with_regex("XYZ-720.mp4"), None);
        assert_eq!(r.recognize_with_regex("ABC-2160.mp4"), None);
    }

    #[test]
    fn recognize_failure() {
        let r = DesignationRecognizer::new();
        assert_eq!(r.recognize_with_regex("random_video.mp4"), None);
        assert_eq!(r.recognize_with_regex("123456.mp4"), None);
    }

    #[test]
    fn priority_and_position() {
        let r = DesignationRecognizer::new();
        // FC2 优先级最高
        assert_eq!(
            r.recognize_with_regex("ABC-123 FC2-PPV-1234567.mp4"),
            Some("FC2-1234567".into())
        );
        // 同优先级取位置靠后
        assert_eq!(r.recognize_with_regex("ABC-123 DEF-456.mp4"), Some("DEF-456".into()));
    }

    // ============ 语义标记 ============

    #[test]
    fn marker_part_suffix_letter() {
        let r = DesignationRecognizer::new();
        let info = r.recognize_detailed("SSIS-001A.mp4").unwrap();
        assert_eq!(info.designation, "SSIS-001");
        assert_eq!(info.markers.part.as_deref(), Some("A"));
    }

    #[test]
    fn marker_part_cd() {
        let r = DesignationRecognizer::new();
        let info = r.recognize_detailed("MIDE-123-CD2.mp4").unwrap();
        assert_eq!(info.designation, "MIDE-123");
        assert_eq!(info.markers.part.as_deref(), Some("CD2"));
    }

    #[test]
    fn marker_chinese_subtitle() {
        let r = DesignationRecognizer::new();
        assert!(r.recognize_detailed("SSIS-001-C.mp4").unwrap().markers.chinese_subtitle);
        assert!(r.recognize_detailed("SSIS-001-ch.mp4").unwrap().markers.chinese_subtitle);
        assert!(r.recognize_detailed("ABC-123中文字幕.mp4").unwrap().markers.chinese_subtitle);
        // 纯番号不应误判
        assert!(!r.recognize_detailed("SSIS-001.mp4").unwrap().markers.chinese_subtitle);
    }

    #[test]
    fn marker_version_and_vr() {
        let r = DesignationRecognizer::new();
        assert_eq!(
            r.recognize_detailed("SSIS-001-4K.mp4").unwrap().markers.resolution.as_deref(),
            Some("4K")
        );
        assert!(r.recognize_detailed("SSIS-001-UC.mp4").unwrap().markers.uncensored);
        assert!(r.recognize_detailed("SSIS-001-LEAK.mp4").unwrap().markers.leaked);
        // VR：厂牌前缀
        assert!(r.recognize_detailed("SIVR-00123.mp4").unwrap().markers.vr);
        // VR：文件标记
        assert!(r.recognize_detailed("[VR] ABC-123.mp4").unwrap().markers.vr);
    }

    #[test]
    fn markers_to_tags() {
        let r = DesignationRecognizer::new();
        let info = r.recognize_detailed("SSIS-001-C-CD1.mp4").unwrap();
        assert_eq!(info.designation, "SSIS-001");
        let tags = info.markers.to_tags();
        assert!(tags.contains(&"中文字幕".to_string()));
        assert!(tags.contains(&"分片CD1".to_string()));
    }

    #[test]
    fn clean_designation_has_no_markers() {
        let r = DesignationRecognizer::new();
        let info = r.recognize_detailed("ABC-123.mp4").unwrap();
        assert!(info.markers.is_empty());
    }

    #[test]
    fn amateur_prefix_not_eaten_by_longer_digit_run() {
        let r = DesignationRecognizer::new();
        // 日期戳直接拼番号：不应把日期数字吞进素人前缀
        assert_eq!(r.recognize_with_regex("20231231SSIS-001.mp4"), Some("SSIS-001".into()));
        // 正常素人仍识别
        assert_eq!(r.recognize_with_regex("390JAC-132.mp4"), Some("390JAC-132".into()));
    }

    #[test]
    fn article_after_space_not_treated_as_part() {
        let r = DesignationRecognizer::new();
        let info = r.recognize_detailed("ABC-123 a movie.mp4").unwrap();
        assert_eq!(info.designation, "ABC-123");
        assert!(info.markers.part.is_none(), "空格后的冠词 a 不应被当成分片");
        // 紧邻后缀字母仍正确
        assert_eq!(r.recognize_detailed("SSIS-001A.mp4").unwrap().markers.part.as_deref(), Some("A"));
    }

    // ============ D2Pass 系纯数字番号（加勒比 / 一本道 / 天然むすめ / 帕高）============

    #[test]
    fn d2pass_keeps_separator_and_reads_studio_tag() {
        let r = DesignationRecognizer::new();
        let info = r.recognize_detailed("110615-001-carib-1080p.mp4").unwrap();
        assert_eq!(info.designation, "110615-001");
        assert_eq!(info.markers.studio.as_deref(), Some("Caribbeancom"));
        assert!(info.is_uncensored);
        assert!(info.markers.part.is_none());

        let info = r.recognize_detailed("110615_001-1pon-1080p.mp4").unwrap();
        assert_eq!(info.designation, "110615_001");
        assert_eq!(info.markers.studio.as_deref(), Some("1Pondo"));

        assert_eq!(
            r.recognize_detailed("110615_001-paco.mp4").unwrap().markers.studio.as_deref(),
            Some("PACOPACOMAMA")
        );
        // 无厂牌缩写：分隔符原样保留，不猜厂牌
        let info = r.recognize_detailed("110615_001.mp4").unwrap();
        assert_eq!(info.designation, "110615_001");
        assert!(info.markers.studio.is_none());
    }

    #[test]
    fn d2pass_studio_tag_corrects_separator() {
        let r = DesignationRecognizer::new();
        // 文件名分隔符写错时以厂牌为准
        assert_eq!(r.recognize_with_regex("110615-001-1pon.mp4"), Some("110615_001".into()));
        assert_eq!(r.recognize_with_regex("110615_001-carib.mp4"), Some("110615-001".into()));
    }

    #[test]
    fn d2pass_recognizes_10musume_two_digit_number() {
        let r = DesignationRecognizer::new();
        let info = r.recognize_detailed("110615_01-10mu-1080p.mp4").unwrap();
        assert_eq!(info.designation, "110615_01");
        assert_eq!(info.markers.studio.as_deref(), Some("10musume"));
        // 两位序号仅认下划线；三位序号不被截成两位
        assert_eq!(r.recognize_with_regex("110615_001.mp4"), Some("110615_001".into()));
        assert_eq!(r.recognize_with_regex("110615-01.mp4"), None);
    }

    #[test]
    fn d2pass_studio_prefix_beats_fake_letter_designation() {
        let r = DesignationRecognizer::new();
        // 厂牌缩写在前：不得截成 CARIB-11061 / PON-11061
        assert_eq!(r.recognize_with_regex("carib-110615-001.mp4"), Some("110615-001".into()));
        assert_eq!(r.recognize_with_regex("1pon-110615_001.mp4"), Some("110615_001".into()));
        assert_eq!(
            r.recognize_with_regex("Caribbeancom 110615-001 title.mp4"),
            Some("110615-001".into())
        );
    }

    #[test]
    fn d2pass_tag_ignored_for_non_numeric_designation() {
        let r = DesignationRecognizer::new();
        // 有码番号旁的 paco 等词不当作厂牌
        let info = r.recognize_detailed("SSIS-001 paco.mp4").unwrap();
        assert_eq!(info.designation, "SSIS-001");
        assert!(info.markers.studio.is_none());
    }

    #[test]
    fn d2pass_rule_mismatch_falls_back_to_generic() {
        let r = DesignationRecognizer::new();
        // 厂牌缩写在但格式不符（天然むすめ应为两位序号）：丢弃厂牌，走通用路径并保留原分隔符
        let info = r.recognize_detailed("110615_001-10mu.mp4").unwrap();
        assert_eq!(info.designation, "110615_001");
        assert!(info.markers.studio.is_none());
    }

    #[test]
    fn d2pass_not_cut_from_longer_digit_run() {
        let r = DesignationRecognizer::new();
        // 时间戳不应被截出伪番号
        assert_eq!(r.recognize_with_regex("20231105_12.mp4"), None);
        assert_eq!(r.recognize_with_regex("20231105_123.mp4"), None);
    }

    #[test]
    fn same_designation_respects_d2pass_separator() {
        assert!(same_designation("110615-001", "110615-001"));
        assert!(same_designation("ssis-001", "SSIS001"));
        assert!(same_designation("110615_001", "110615_001"));
        // 加勒比与一本道同日同号是不同影片
        assert!(!same_designation("110615-001", "110615_001"));
        assert!(!same_designation("110615-001", "110615-002"));
    }

    // ============ 分段解析（stacking）============

    #[test]
    fn parse_stack_part_recognizes_numeric_suffixes() {
        // 用户实际场景：PartNNN（含前导零）
        assert_eq!(parse_stack_part("SSIS-724Part002"), Some(("SSIS-724".into(), 2)));
        assert_eq!(parse_stack_part("SSIS-724Part001"), Some(("SSIS-724".into(), 1)));
        assert_eq!(parse_stack_part("SSIS-724Part003"), Some(("SSIS-724".into(), 3)));
        // 各种分隔符与单位
        assert_eq!(parse_stack_part("ABC-123-CD2"), Some(("ABC-123".into(), 2)));
        assert_eq!(parse_stack_part("ABC-123.disc1"), Some(("ABC-123".into(), 1)));
        assert_eq!(parse_stack_part("ABC-123 part 3"), Some(("ABC-123".into(), 3)));
        assert_eq!(parse_stack_part("ABC-123_pt2"), Some(("ABC-123".into(), 2)));
        assert_eq!(parse_stack_part("ABC-123 VOL.2"), Some(("ABC-123".into(), 2)));
        // 中文
        assert_eq!(parse_stack_part("ABC-123分卷2"), Some(("ABC-123".into(), 2)));
        assert_eq!(parse_stack_part("ABC-123第3部"), Some(("ABC-123".into(), 3)));
    }

    #[test]
    fn parse_stack_part_recognizes_trailing_letters() {
        assert_eq!(parse_stack_part("SSIS-001A"), Some(("SSIS-001".into(), 1)));
        assert_eq!(parse_stack_part("SSIS-001B"), Some(("SSIS-001".into(), 2)));
        // C 是中文字幕标记，不算分段
        assert_eq!(parse_stack_part("SSIS-001C"), None);
    }

    #[test]
    fn parse_stack_part_recognizes_bare_trailing_number() {
        // FC2-PPV 番号后接裸数字分段（用户实际场景）
        assert_eq!(parse_stack_part("FC2-PPV-2458342-1"), Some(("FC2-PPV-2458342".into(), 1)));
        assert_eq!(parse_stack_part("FC2-PPV-2458342-2"), Some(("FC2-PPV-2458342".into(), 2)));
        // 标准番号后接裸数字（含前导零），两段应归并到同一基名
        assert_eq!(parse_stack_part("STARS-818-01"), Some(("STARS-818".into(), 1)));
        assert_eq!(parse_stack_part("STARS-818-02"), Some(("STARS-818".into(), 2)));
        // 其他分隔符与前导零
        assert_eq!(parse_stack_part("ABC-123-2"), Some(("ABC-123".into(), 2)));
        assert_eq!(parse_stack_part("ABC-123_2"), Some(("ABC-123".into(), 2)));
        assert_eq!(parse_stack_part("ABC-123-02"), Some(("ABC-123".into(), 2)));
        // 不含连字符的纯番号/纯数字番号不应被裸数字规则误拆
        assert_eq!(parse_stack_part("ABC-123"), None);
        assert_eq!(parse_stack_part("SSIS-001"), None);
        assert_eq!(parse_stack_part("123456-789"), None);
        assert_eq!(parse_stack_part("010120-001"), None);
        // 无分段的完整 FC2 番号（数字段过长，不会被当作分段序号）
        assert_eq!(parse_stack_part("FC2-PPV-2458342"), None);
        // 无分段的完整标准番号（单连字符，不折叠）
        assert_eq!(parse_stack_part("STARS-818"), None);
    }

    #[test]
    fn parse_stack_part_recognizes_d2pass_quality_segments() {
        // 用户实际场景：加勒比官方分段命名 fhd1 / fhd2（同目录两个文件），基名含厂牌词
        assert_eq!(parse_stack_part("012413-001-carib-fhd1"), Some(("012413-001-CARIB".into(), 1)));
        assert_eq!(parse_stack_part("012413-001-carib-fhd2"), Some(("012413-001-CARIB".into(), 2)));
        // 其它画质词写法
        assert_eq!(parse_stack_part("110615_001-1pon-whole_hd1"), Some(("110615_001-1PON".into(), 1)));
        assert_eq!(parse_stack_part("110615-001-carib-high_3"), Some(("110615-001-CARIB".into(), 3)));
        assert_eq!(parse_stack_part("ABC-123-HD2"), Some(("ABC-123".into(), 2)));
        // 整片 / 分辨率后缀不是分段
        assert_eq!(parse_stack_part("012413-001-carib-1080p"), None);
        assert_eq!(parse_stack_part("SSIS-001-HD720"), None);
        assert_eq!(parse_stack_part("122720-001-carib"), None);
    }

    #[test]
    fn parse_stack_part_ignores_non_stack_names() {
        // 纯番号无分段后缀
        assert_eq!(parse_stack_part("SSIS-724"), None);
        assert_eq!(parse_stack_part("300MAAN-783"), None);
        assert_eq!(parse_stack_part("FC2-PPV-1234567"), None);
        assert_eq!(parse_stack_part("MKBD-S94"), None);
        assert_eq!(parse_stack_part("XXX-AV-20845"), None);
        // "cd" 恰在词中但其后非分段序号
        assert_eq!(parse_stack_part("ABCD-123"), None);
        // "pt" 贴在字母词尾（无分隔符）不应误判
        assert_eq!(parse_stack_part("SCRIPT3"), None);
    }

    #[test]
    fn parse_stack_part_groups_same_base() {
        // 三段应归并到同一基名，序号各异
        let a = parse_stack_part("SSIS-724Part001").unwrap();
        let b = parse_stack_part("SSIS-724Part002").unwrap();
        let c = parse_stack_part("SSIS-724Part003").unwrap();
        assert_eq!(a.0, b.0);
        assert_eq!(b.0, c.0);
        assert_eq!((a.1, b.1, c.1), (1, 2, 3));
    }

    // ============ 分段范围目录 ============

    #[test]
    fn stack_scope_dir_climbs_dirs_named_after_the_stack() {
        // 用户实际布局：第 1 段在 SIVR-015-1/，第 2 段被归入其下的 SIVR-015-2/ 子目录，
        // 两段的范围都应回溯到容纳整组的 05/
        assert_eq!(stack_scope_dir("/lib/05/SIVR-015-1", "SIVR-015"), "/lib/05");
        assert_eq!(stack_scope_dir("/lib/05/SIVR-015-1/SIVR-015-2", "SIVR-015"), "/lib/05");
        // 平级的按段命名目录
        assert_eq!(stack_scope_dir("/lib/05/SIVR-015-2", "SIVR-015"), "/lib/05");
        // 基名目录（标准布局）同样回溯
        assert_eq!(stack_scope_dir("/lib/05/SIVR-015", "SIVR-015"), "/lib/05");
        // 目录名大小写 / 显式分段词
        assert_eq!(stack_scope_dir("/lib/06/savr-1111-1", "SAVR-1111"), "/lib/06");
        assert_eq!(stack_scope_dir("/lib/02/HMN-372Part001", "HMN-372"), "/lib/02");
    }

    #[test]
    fn stack_scope_dir_stops_at_unrelated_dir() {
        // 平铺在普通目录：范围就是所在目录
        assert_eq!(stack_scope_dir("/lib/07", "FC2-388787"), "/lib/07");
        // 目录名是别的番号的分段 / 相似番号：不回溯
        assert_eq!(stack_scope_dir("/lib/05/SIVR-016-1", "SIVR-015"), "/lib/05/SIVR-016-1");
        assert_eq!(stack_scope_dir("/lib/05/SIVR-015", "SIVR-0150"), "/lib/05/SIVR-015");
    }

    #[test]
    fn dir_belongs_to_stack_matches_base_and_parts() {
        assert!(dir_belongs_to_stack("SIVR-015", "SIVR-015"));
        assert!(dir_belongs_to_stack("sivr-015-2", "SIVR-015"));
        assert!(dir_belongs_to_stack("SIVR-015-CD1", "SIVR-015"));
        assert!(!dir_belongs_to_stack("SIVR-016-1", "SIVR-015"));
        assert!(!dir_belongs_to_stack("05", "SIVR-015"));
    }
}
