use super::one;
use crate::types::Rule;

pub(super) const RULES: &[Rule] = &[Rule::new(
    "switch_lang",
    |c| {
        c.output().contains("not found")
            && (has_korean(&c.script)
                || translate(&c.script).is_some_and(|script| script != c.ctx().alias))
    },
    |c| translate(&decompose_korean(&c.script)).map_or_else(Vec::new, one),
)];

const TARGET: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,./QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>?";
const LAYOUTS: &[&str] = &[
    "йцукенгшщзхъфывапролджэячсмитьбю.ЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮ,",
    "йцукенгшщзхїфівапролджєячсмитьбю.ЙЦУКЕНГШЩЗХЇФІВАПРОЛДЖЄЯЧСМИТЬБЮ,",
    "ضصثقفغعهخحجچشسیبلاتنمکگظطزرذدپو./ًٌٍَُِّْ][}{ؤئيإأآة»«:؛كٓژٰ‌ٔء><؟",
    "/'קראטוןםפ][שדגכעיחלךף,זסבהנמצתץ.QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>?",
    "ㅂㅈㄷㄱㅅㅛㅕㅑㅐㅔ[]ㅁㄴㅇㄹㅎㅗㅓㅏㅣ;'ㅋㅌㅊㅍㅠㅜㅡ,./ㅃㅉㄸㄲㅆㅛㅕㅑㅒㅖ{}ㅁㄴㅇㄹㅎㅗㅓㅏㅣ:\"ㅋㅌㅊㅍㅠㅜㅡ<>?",
];
const GREEK: &str =
    ";ςερτυθιοπ[]ασδφγηξκλ΄ζχψωβνμ,./:΅ΕΡΤΥΘΙΟΠ{}ΑΣΔΦΓΗΞΚΛ¨\"ΖΧΨΩΒΝΜ<>?άέύίόήώΆΈΎΊΌΉΏ";
const GREEK_TARGET: &str =
    "qwertyuiop[]asdfghjkl'zxcvbnm,./QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>?aeyiohvAEYIOHV";

fn has_korean(script: &str) -> bool {
    script
        .chars()
        .any(|ch| matches!(ch, 'ㄱ'..='ㅎ' | 'ㅏ'..='ㅣ' | '가'..='힣'))
}

fn translate(script: &str) -> Option<String> {
    let valid = |layout: &str| {
        script
            .chars()
            .all(|ch| matches!(ch, ' ' | '-' | '_') || layout.contains(ch))
    };
    if let Some(layout) = LAYOUTS.iter().find(|layout| valid(layout)) {
        return Some(
            script
                .chars()
                .map(|ch| {
                    layout
                        .chars()
                        .position(|key| key == ch)
                        .and_then(|index| TARGET.chars().nth(index))
                        .unwrap_or(ch)
                })
                .collect(),
        );
    }
    if valid(GREEK) {
        return Some(
            script
                .chars()
                .map(|ch| {
                    GREEK
                        .chars()
                        .position(|key| key == ch)
                        .and_then(|index| GREEK_TARGET.chars().nth(index))
                        .unwrap_or(ch)
                })
                .collect(),
        );
    }
    None
}

/// Expand Hangul syllables and compound jamo into the keys that produced them.
fn decompose_korean(script: &str) -> String {
    const HEAD: &str = "ㄱㄲㄴㄷㄸㄹㅁㅂㅃㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ";
    const BODY: &str = "ㅏㅐㅑㅒㅓㅔㅕㅖㅗㅘㅙㅚㅛㅜㅝㅞㅟㅠㅡㅢㅣ";
    const TAIL: &str = " ㄱㄲㄳㄴㄵㄶㄷㄹㄺㄻㄼㄽㄾㄿㅀㅁㅂㅄㅅㅆㅇㅈㅊㅋㅌㅍㅎ";
    let mut keys = String::new();
    for ch in script.chars() {
        if matches!(ch, '가'..='힣') {
            let index = ch as usize - '가' as usize;
            for key in [
                HEAD.chars().nth(index / 588),
                BODY.chars().nth(index % 588 / 28),
                TAIL.chars().nth(index % 28),
            ]
            .into_iter()
            .flatten()
            .filter(|ch| *ch != ' ')
            {
                append_jamo(&mut keys, key);
            }
        } else {
            append_jamo(&mut keys, ch);
        }
    }
    keys
}

fn append_jamo(keys: &mut String, ch: char) {
    let compound = match ch {
        'ㅘ' => "ㅗㅏ",
        'ㅙ' => "ㅗㅐ",
        'ㅚ' => "ㅗㅣ",
        'ㅝ' => "ㅜㅓ",
        'ㅞ' => "ㅜㅔ",
        'ㅟ' => "ㅜㅣ",
        'ㅢ' => "ㅡㅣ",
        'ㄳ' => "ㄱㅅ",
        'ㄵ' => "ㄴㅈ",
        'ㄶ' => "ㄴㅎ",
        'ㄺ' => "ㄹㄱ",
        'ㄻ' => "ㄹㅁ",
        'ㄼ' => "ㄹㅂ",
        'ㄽ' => "ㄹㅅ",
        'ㄾ' => "ㄹㅌ",
        'ㅀ' => "ㄹㅎ",
        'ㅄ' => "ㅂㅅ",
        _ => {
            keys.push(ch);
            return;
        }
    };
    keys.push_str(compound);
}
