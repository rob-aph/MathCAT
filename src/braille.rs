#![allow(clippy::needless_return)]
use sxd_document::dom::Element;
use sxd_document::Package;
use crate::errors::*;
use crate::pretty_print::mml_to_string;
use crate::prefs::PreferenceManager;
use std::cell::Ref;
use regex::{Captures, Regex, RegexSet};
use phf::{phf_map, phf_set};
use crate::speech::{BRAILLE_RULES, SpeechRulesWithContext};
use std::ops::Range;

static UEB_PREFIXES: phf::Set<char> = phf_set! {
    '⠼', '⠈', '⠘', '⠸', '⠐', '⠨', '⠰', '⠠',
};


/// braille the MathML
/// If 'nav_node_id' is not an empty string, then the element with that id will have dots 7 & 8 turned on as per the pref
pub fn braille_mathml(mathml: Element, nav_node_id: &str) -> Result<String> {
    crate::speech::SpeechRules::update()?;
    return BRAILLE_RULES.with(|rules| {
        rules.borrow_mut().read_files()?;
        let rules = rules.borrow();
        let new_package = Package::new();
        let mut rules_with_context = SpeechRulesWithContext::new(&rules, new_package.as_document(), nav_node_id);
        let braille_string = rules_with_context.match_pattern::<String>(mathml)
                        .chain_err(|| "Pattern match/replacement failure!")?;
        let braille_string = braille_string.replace(' ', "");
        let pref_manager = rules_with_context.get_rules().pref_manager.borrow();
        let highlight_style = pref_manager.pref_to_string("BrailleNavHighlight");
        let braille_code = pref_manager.pref_to_string("BrailleCode");
        let braille = match braille_code.as_str() {
            "Nemeth" => nemeth_cleanup(braille_string),
            "UEB" => ueb_cleanup(pref_manager, braille_string),
            "Vietnam" => vietnam_cleanup(pref_manager, braille_string),   // FIX: probably needs some specialized cleanup
            "CMU" => cmu_cleanup(pref_manager, braille_string),   // FIX: probably needs some specialized cleanup
            _ => braille_string,    // probably needs cleanup if someone has another code, but this will have to get added by hand
        };

        return Ok(
            if highlight_style != "Off" {
                highlight_braille_chars(braille, &braille_code, highlight_style == "All")
            } else {
             braille
            }
        );
    });

    // highlight with dots 7 & 8 based on the highlight style
    // both the start and stop points will be extended to deal with indicators such as capitalization
    // if 'fill_range' is true, the interior will be highlighted
    fn highlight_braille_chars(braille: String, braille_code: &str, fill_range: bool) -> String {
        let mut braille = braille;
        // some special (non-braille) chars weren't converted to having dots 7 & 8 to indicate navigation position
        // they need to be added to the start

        // find start and end indexes of the highlighted region
        let start = braille.find(is_highlighted);
        let end = braille.rfind(is_highlighted);
        if start.is_none() {
            assert!(end.is_none());
            return braille;
        };

        let end = end.unwrap();         // always exists if start exists
        let start = highlight_first_indicator(&mut braille, braille_code, start.unwrap(), end);

        if start == end {
            return braille;
        }

        if !fill_range {
            return braille;
        }

        let mut result = String::with_capacity(braille.len());
        result.push_str(&braille[..start]);
        let highlight_region =&mut braille[start..end];
        for ch in highlight_region.chars() {
            result.push( highlight(ch) );
        };
        result.push_str(&braille[end..]);
        return result;

        fn highlight_first_indicator(braille: &mut String, braille_code: &str, start_index: usize, end_index: usize) -> usize {
            // chars in the braille block range use 3 bytes -- we can use that to optimize the code some
            let first_ch = unhighlight(braille[start_index..start_index+3].chars().next().unwrap());

            // need to highlight (optional) capital/number, language, and style (max 2 chars) also in that (rev) order
            let prefix_ch_index = std::cmp::max(0, start_index as isize - 5*3) as usize;
            let indicators = &braille[prefix_ch_index..start_index];   // chars to be examined
            let i_byte_start = start_index - 3 * match braille_code {
                "Nemeth" => i_start_nemeth(indicators, first_ch),
                "UEB" => i_start_ueb(indicators),
                _ => {
                    error!("highlight_first_indicator: Unknown braille code '{}'", braille);
                    0
                },
            };
            if i_byte_start < start_index {
                // remove old highlight as long as we don't wipe out the end highlight
                if start_index < end_index {
                    let old_first_char_bytes = start_index..start_index+3;
                    let replacement_str = unhighlight(braille[old_first_char_bytes.clone()].chars().next().unwrap()).to_string();
                    braille.replace_range(old_first_char_bytes, &replacement_str);
                }

                // add new highlight
                let new_first_char_bytes = i_byte_start..i_byte_start+3;
                let replacement_str = highlight(braille[new_first_char_bytes.clone()].chars().next().unwrap()).to_string();
                braille.replace_range(new_first_char_bytes, &replacement_str);
            }

            return i_byte_start;
        }

    }

    /// Given a position in a Nemeth string, what is the position character that starts it (e.g, the prev char for capital letter)
    fn i_start_nemeth(braille_prefix: &str, first_ch: char) -> usize {
        static NEMETH_NUMBERS: phf::Set<char> = phf_set! {
            '⠂', '⠆', '⠒', '⠲', '⠢', '⠖', '⠶', '⠦', '⠔', '⠴', '⠨' // 1, 2, ...9, 0, decimal pt
        };
        let mut n_chars = 0;
        let prefix = &mut braille_prefix.chars().rev().peekable();
        if prefix.peek() == Some(&'⠠') ||  // cap indicator
           (prefix.peek() == Some(&'⠼') && NEMETH_NUMBERS.contains(&first_ch)) ||  // number indicator
           [Some(&'⠸'), Some(&'⠈'), Some(&'⠨')].contains(&prefix.peek()) {         // bold, script/blackboard, italic indicator
            n_chars += 1;
            prefix.next();
        } 

        if [Some(&'⠰'), Some(&'⠸'), Some(&'⠨')].contains(&prefix.peek()) {   // English, German, Greek
            n_chars += 1;
        } else if prefix.peek() == Some(&'⠈') {  
            let ch = prefix.next();                              // Russian/Greek Variant
            if ch == Some('⠈') || ch == Some('⠨') {
                n_chars += 2;
            }
        } else if prefix.peek() == Some(&'⠠')  { // Hebrew 
            let ch = prefix.next();                              // Russian/Greek Variant
            if ch == Some('⠠') {
                n_chars += 2;
            }
        };
        return n_chars;
    }

    /// Given a position in a UEB string, what is the position character that starts it (e.g, the prev char for capital letter)
    fn i_start_ueb(braille_prefix: &str) -> usize {
        let prefix = &mut braille_prefix.chars().rev().peekable();
        let mut n_chars = 0;
        while let Some(ch) = prefix.next() {
            if UEB_PREFIXES.contains(&ch) {
                n_chars += 1;
            } else if ch == '⠆' {
                let n_typeform_chars = check_for_typeform(prefix);
                if n_typeform_chars > 0 {
                    n_chars += n_typeform_chars;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        return n_chars;
    }

    fn check_for_typeform(prefix: &mut dyn std::iter::Iterator<Item=char>) -> usize {
        static UEB_TYPEFORM_PREFIXES: phf::Set<char> = phf_set! {
            '⠈', '⠘', '⠸', '⠨',
        };

        if let Some(typeform_indicator) = prefix.next() {
            if UEB_TYPEFORM_PREFIXES.contains(&typeform_indicator) {
                return 2;
            } else if typeform_indicator == '⠼' {
                if let Some(user_defined_typeform_indicator) = prefix.next() {
                    if UEB_TYPEFORM_PREFIXES.contains(&user_defined_typeform_indicator) || user_defined_typeform_indicator == '⠐' {
                        return 3;
                    }
                }
            }
        }
        return 0;
    }
}

fn is_highlighted(ch: char) -> bool {
    let ch_as_u32 = ch as u32;
    return (0x28C0..0x28FF).contains(&ch_as_u32);
}

fn highlight(ch: char) -> char {
    return unsafe{char::from_u32_unchecked(ch as u32 | 0xC0)};      
}

fn unhighlight(ch: char) -> char {
    let ch_as_u32 = ch as u32;
    if (0x28C0..0x28FF).contains(&ch_as_u32) {
        return unsafe{char::from_u32_unchecked(ch_as_u32 & 0x283F)};  
    } else {
        return ch;
    }
}


fn nemeth_cleanup(raw_braille: String) -> String {
    // Typeface: S: sans-serif, B: bold, T: script/blackboard, I: italic, R: Roman
    // Language: E: English, D: German, G: Greek, V: Greek variants, H: Hebrew, U: Russian
    // Indicators: C: capital, N: number, P: punctuation, M: multipurpose
    // Others:
    //      W -- whitespace that should be kept (e.g, in a numeral)
    //      𝑁 -- hack for special case of a lone decimal pt -- not considered a number but follows rules mostly 
    // SRE doesn't have H: Hebrew or U: Russian, so not encoded (yet)
    // Note: some "positive" patterns find cases to keep the char and transform them to the lower case version
    static NEMETH_INDICATOR_REPLACEMENTS: phf::Map<&str, &str> = phf_map! {
        "S" => "⠈⠰",    // sans-serif
        "B" => "⠸",     // bold
        "𝔹" => "⠈",     // blackboard
        "T" => "⠈",     // script (mapped to be the same a blackboard)
        "I" => "⠨",     // italic
        "R" => "",      // roman
        "E" => "⠰",     // English
        "D" => "⠸",     // German (Deutsche)
        "G" => "⠨",     // Greek
        "V" => "⠨⠈",    // Greek Variants
        "H" => "⠠⠠",    // Hebrew
        "U" => "⠈⠈",    // Russian
        "C" => "⠠",     // capital
        "P" => "⠸",     // punctuation
        "𝐏" => "⠸",     // hack for punctuation after a roman numeral -- never removed
        "L" => "",      // letter
        "l" => "",      // letter inside enclosed list
        "M" => "",      // multipurpose indicator
        "m" => "⠐",     // required multipurpose indicator
        "N" => "",      // potential number indicator before digit
        "n" => "⠼",     // required number indicator before digit
        "𝑁" => "",      // hack for special case of a lone decimal pt -- not considered a number but follows rules mostly
        "W" => "⠀",     // whitespace
        "w" => "⠀",     // whitespace from comparison operator
        "," => "⠠⠀",    // comma
        "b" => "⠐",     // baseline
        "↑" => "⠘",     // superscript
        "↓" => "⠰",     // subscript
    };

    lazy_static! {
        // Add an English Letter indicator. This involves finding "single letters".
        // The green book has a complicated set of cases, but the Nemeth UEB Rule book (May 2020), 4.10 has a much shorter explanation:
        //   punctuation or whitespace on the left and right ignoring open/close chars
        //   https://nfb.org/sites/www.nfb.org/files/files-pdf/braille-certification/lesson-4--provisional-5-9-20.pdf
        static ref ADD_ENGLISH_LETTER_INDICATOR: Regex = 
            Regex::new(r"(?P<start>^|W|P.[\u2800-\u28FF]?|,)(?P<open>[\u2800-\u28FF]?⠷)?(?P<letter>C?L.)(?P<close>[\u2800-\u28FF]?⠾)?(?P<end>W|P|,|$)").unwrap();
        
        // Trim braille spaces before and after braille indicators
        // In order: fraction, /, cancellation, letter, baseline
        // Note: fraction over is not listed due to example 42(4) which shows a space before the "/"
        static ref REMOVE_SPACE_BEFORE_BRAILLE_INDICATORS: Regex = 
            Regex::new(r"(⠄⠄⠄|⠤⠤⠤⠤)[Ww]+([⠼⠸⠪])").unwrap();
        static ref REMOVE_SPACE_AFTER_BRAILLE_INDICATORS: Regex = 
            Regex::new(r"([⠹⠻Llb])[Ww]+(⠄⠄⠄|⠤⠤⠤⠤)").unwrap();

        // Hack to convert non-numeric '.' to numeric '.'
        // The problem is that the numbers are hidden inside of mover -- this might be more general than rule 99_2.
        static ref DOTS_99_A_2: Regex = Regex::new(r"𝑁⠨mN").unwrap();

        // Punctuation is one or two chars. There are (currently) only 3 2-char punct chars (—‘’) -- we explicitly list them below
        static ref REMOVE_SPACE_BEFORE_PUNCTUATION_151: Regex = 
            Regex::new(r"w(P.[⠤⠦⠠]?|[\u2800-\u28FF]?⠾)").unwrap();
        static ref REMOVE_SPACE_AFTER_PUNCTUATION_151: Regex = 
            Regex::new(r"(P.[⠤⠦⠠]?|[\u2800-\u28FF]?⠷)w").unwrap();

        // Multipurpose indicator insertion
        // 149 -- consecutive comparison operators have no space -- instead a multipurpose indicator is used (doesn't require a regex)

        // 177.2 -- add after a letter and before a digit (or decimal pt) -- digits will start with N
        static ref MULTI_177_2: Regex = 
            Regex::new(r"([Ll].)[N𝑁]").unwrap();

        // keep between numeric subscript and digit ('M' added by subscript rule)
        static ref MULTI_177_3: Regex = 
            Regex::new(r"([N𝑁].)M([N𝑁].)").unwrap(); 

        // Add after decimal pt for non-digits except for comma and punctuation
        // Note: since "." can be in the middle of a number, there is not necessarily a "N"
        // Although not mentioned in 177_5, don't add an 'M' before an 'm'
        static ref MULTI_177_5: Regex = 
            Regex::new(r"([N𝑁]⠨)([^⠂⠆⠒⠲⠢⠖⠶⠦⠔N𝑁,Pm])").unwrap(); 


        // Pattern for rule II.9a (add numeric indicator at start of line or after a space)
        // 1. start of line
        // 2. optional minus sign (⠤)
        // 3. optional typeface indicator
        // 4. number (N)
        static ref NUM_IND_9A: Regex = 
            Regex::new(r"(?P<start>^|[,Ww])(?P<minus>⠤?)N").unwrap();  

        // Needed after section mark(§), paragraph mark(¶), #, or *
        static ref NUM_IND_9C: Regex = 
            Regex::new(r"(⠤?)(⠠⠷|⠠⠳|⠠⠈⠷)N").unwrap();  

        // Needed after section mark(§), paragraph mark(¶), #, or *
        static ref NUM_IND_9D: Regex = 
            Regex::new(r"(⠈⠠⠎|⠈⠠⠏|⠨⠼|⠈⠼)N").unwrap();  

        // Needed after a typeface change or interior shape modifier indicator
        static ref NUM_IND_9E: Regex = Regex::new(r"(?P<face>[SBTIR]+?)N").unwrap();  
        static ref NUM_IND_9E_SHAPE: Regex = Regex::new(r"(?P<mod>⠸⠫)N").unwrap();  

        // Needed after hyphen that follows a word, abbreviation, or punctuation (caution about rule 11d)
        // Note -- hyphen might encode as either "P⠤" or "⠤" depending on the tag used
        static ref NUM_IND_9F: Regex = Regex::new(r"([Ll].[Ll].|P.)(P?⠤)N").unwrap();  

        // Enclosed list exception
        // Normally we don't add numeric indicators in enclosed lists (done in get_braille_nemeth_chars).
        // The green book says "at the start" of an item, don't add the numeric indicator.
        // The NFB list exceptions after function abbreviations and angles, but what this really means is "after a space"
        static ref NUM_IND_ENCLOSED_LIST: Regex = Regex::new(r"w([⠂⠆⠒⠲⠢⠖⠶⠦⠔⠴])").unwrap();  

        // Punctuation chars (Rule 38.6 says don't use before ",", "hyphen", "-", "…")
        // Never use punctuation indicator before these (38-6)
        //      "…": "⠀⠄⠄⠄"
        //      "-": "⠸⠤" (hyphen and dash)
        //      ",": "⠠⠀"     -- spacing already added
        // Rule II.9b (add numeric indicator after punctuation [optional minus[optional .][digit]
        //  because this is run after the above rule, some cases are already caught, so don't
        //  match if there is already a numeric indicator
        static ref NUM_IND_9B: Regex = Regex::new(r"(?P<punct>P..?)(?P<minus>⠤?)N").unwrap();  

        // Before 79b (punctuation)
        static ref REMOVE_LEVEL_IND_BEFORE_SPACE_COMMA_PUNCT: Regex = Regex::new(r"(?:[↑↓]+b?|b)([Ww,P]|$)").unwrap();

        static ref REMOVE_LEVEL_IND_BEFORE_BASELINE: Regex = Regex::new(r"(?:[↑↓]+b)").unwrap();

        // Except for the four chars above, the unicode rules always include a punctuation indicator.
        // The cases to remove them (that seem relevant to MathML) are:
        //   Beginning of line or after a space (V 38.1)
        //   After a word (38.4)
        //   2nd or subsequent punctuation (includes, "-", etc) (38.7)
        static ref REMOVE_AFTER_PUNCT_IND: Regex = Regex::new(r"(^|[Ww]|[Ll].[Ll].)P(.)").unwrap();  
        static ref REPLACE_INDICATORS: Regex =Regex::new(r"([SB𝔹TIREDGVHUP𝐏CLlMmb↑↓Nn𝑁Ww,])").unwrap();          
        static ref COLLAPSE_SPACES: Regex = Regex::new(r"⠀⠀+").unwrap();
    }

  debug!("Before:  \"{}\"", raw_braille);
    // replacements might overlap at boundaries (e.g., whitespace) -- need to repeat
    let mut start = 0;
    let mut result = String::with_capacity(raw_braille.len()+ raw_braille.len()/4);  // likely upper bound
    while let Some(matched) = ADD_ENGLISH_LETTER_INDICATOR.find_at(&raw_braille, start) {
        result.push_str(&raw_braille[start..matched.start()]);
        let replacement = ADD_ENGLISH_LETTER_INDICATOR.replace(
                &raw_braille[matched.start()..matched.end()], "${start}${open}E${letter}${close}");
        // debug!("matched='{}', start/end={}/{}; replacement: {}", &raw_braille[matched.start()..matched.end()], matched.start(), matched.end(), replacement);
        result.push_str(&replacement);
        // put $end back on because needed for next match (e.g., whitespace at end and then start of next match)
        // but it could also match because it was at the end, in which case "-1" is wrong -- tested after loop for that
        start = matched.end() - 1;
    }
    if !raw_braille.is_empty() && ( start < raw_braille.len()-1 || "WP,".contains(raw_braille.chars().nth_back(0).unwrap()) ) {       // see comment about $end above
        result.push_str(&raw_braille[start..]);
    }
  debug!("ELIs:    \"{}\"", result);  

    let result = NUM_IND_ENCLOSED_LIST.replace_all(&result, "wn${1}");

    // Remove blanks before and after braille indicators
    let result = REMOVE_SPACE_BEFORE_BRAILLE_INDICATORS.replace_all(&result, "$1$2");
    let result = REMOVE_SPACE_AFTER_BRAILLE_INDICATORS.replace_all(&result, "$1$2");

    let result = REMOVE_SPACE_BEFORE_PUNCTUATION_151.replace_all(&result, "$1");
    let result = REMOVE_SPACE_AFTER_PUNCTUATION_151.replace_all(&result, "$1");
  debug!("spaces:  \"{}\"", result);

    let result = DOTS_99_A_2.replace_all(&result, "N⠨mN");

    // Multipurpose indicator
    let result = result.replace("ww", "m"); // 149
    let result = MULTI_177_2.replace_all(&result, "${1}m${2}");
    let result = MULTI_177_3.replace_all(&result, "${1}m$2");
    let result = MULTI_177_5.replace_all(&result, "${1}m$2");
  debug!("MULTI:   \"{}\"", result);

    let result = NUM_IND_9A.replace_all(&result, "${start}${minus}n");
    let result = NUM_IND_9C.replace_all(&result, "${1}${2}n");
    let result = NUM_IND_9D.replace_all(&result, "${1}n");
    let result = NUM_IND_9E.replace_all(&result, "${face}n");
    let result = NUM_IND_9E_SHAPE.replace_all(&result, "${mod}n");
    let result = NUM_IND_9F.replace_all(&result, "${1}${2}n");

  debug!("IND_9F:  \"{}\"", result);

    // 9b: insert after punctuation (optional minus sign)
    // common punctuation adds a space, so 9a handled it. Here we deal with other "punctuation" 
    // FIX other punctuation and reference symbols (9d)
    let result = NUM_IND_9B.replace_all(&result, "$punct${minus}n");
//   debug!("A PUNCT: \"{}\"", &result);

    // strip level indicators
    // checks for punctuation char, so needs to before punctuation is stripped.
    
    let result = REMOVE_LEVEL_IND_BEFORE_SPACE_COMMA_PUNCT.replace_all(&result, "$1");
//   debug!("Punct  : \"{}\"", &result);
    let result = REMOVE_LEVEL_IND_BEFORE_BASELINE.replace_all(&result, "b");
//   debug!("Bseline: \"{}\"", &result);

    let result = REMOVE_AFTER_PUNCT_IND.replace_all(&result, "$1$2");
  debug!("Punct38: \"{}\"", &result);

    let result = REPLACE_INDICATORS.replace_all(&result, |cap: &Captures| {
        match NEMETH_INDICATOR_REPLACEMENTS.get(&cap[0]) {
            None => {error!("REPLACE_INDICATORS and NEMETH_INDICATOR_REPLACEMENTS are not in sync"); ""},
            Some(&ch) => ch,
        }
    });

    // Remove unicode blanks at start and end -- do this after the substitutions because ',' introduces spaces
    let result = result.trim_start_matches('⠀').trim_end_matches('⠀');
    let result = COLLAPSE_SPACES.replace_all(result, "⠀");
   
    return result.to_string();

}

// Typeface: S: sans-serif, B: bold, T: script/blackboard, I: italic, R: Roman
// Language: E: English, D: German, G: Greek, V: Greek variants, H: Hebrew, U: Russian
// Indicators: C: capital, N: number, P: punctuation, M: multipurpose
// Others:
//      W -- whitespace that should be kept (e.g, in a numeral)
//      𝑁 -- hack for special case of a lone decimal pt -- not considered a number but follows rules mostly 
// Note: some "positive" patterns find cases to keep the char and transform them to the lower case version
static UEB_INDICATOR_REPLACEMENTS: phf::Map<&str, &str> = phf_map! {
    "S" => "XXX",    // sans-serif -- from prefs
    "B" => "⠘",     // bold
    "𝔹" => "XXX",     // blackboard -- from prefs
    "T" => "⠈",     // script
    "I" => "⠨",     // italic
    "R" => "",      // roman
    // "E" => "⠰",     // English
    "1" => "⠰",      // Grade 1 symbol
    "𝟙" => "⠰⠰",     // Grade 1 word
    "L" => "",       // Letter left in to assist in locating letters
    "D" => "XXX",    // German (Deutsche) -- from prefs
    "G" => "⠨",      // Greek
    "V" => "⠨⠈",     // Greek Variants
    // "H" => "⠠⠠",  // Hebrew
    // "U" => "⠈⠈",  // Russian
    "C" => "⠠",      // capital
    "𝐶" => "⠠",      // capital that never should get word indicator (from chemical element)
    "N" => "⠼",     // number indicator
    "t" => "⠱",     // shape terminator
    "W" => "⠀",     // whitespace
    "𝐖"=> "⠀",     // whitespace
    "s" => "⠆",     // typeface single char indicator
    "w" => "⠂",     // typeface word indicator
    "e" => "⠄",     // typeface & capital terminator 
    "o" => "",       // flag that what follows is an open indicator (used for standing alone rule)
    "c" => "",       // flag that what follows is an close indicator (used for standing alone rule)
    "b" => "",       // flag that what follows is an open or close indicator (used for standing alone rule)
    "," => "⠂",     // comma
    "." => "⠲",     // period
    "-" => "-",     // hyphen
    "—" => "⠠⠤",   // normal dash (2014) -- assume all normal dashes are unified here [RUEB appendix 3]
    "―" => "⠐⠠⠤",  // long dash (2015) -- assume all long dashes are unified here [RUEB appendix 3]
    "#" => "",      // signals end of script
    // '(', '{', '[', '"', '\'', '“', '‘', '«',    // opening chars
    // ')', '}', ']', '\"', '\'', '”', '’', '»',           // closing chars
    // ',', ';', ':', '.', '…', '!', '?'                    // punctuation           

};

// static LETTERS: phf::Set<char> = phf_set! {
//     '⠁', '⠃', '⠉', '⠙', '⠑', '⠋', '⠛', '⠓', '⠊', '⠚', '⠅', '⠇', '⠍', 
//     '⠝', '⠕', '⠏', '⠟', '⠗', '⠎', '⠞', '⠥', '⠧', '⠺', '⠭', '⠽', '⠵',
// };

static LETTER_NUMBERS: phf::Set<char> = phf_set! {
    '⠁', '⠃', '⠉', '⠙', '⠑', '⠋', '⠛', '⠓', '⠊', '⠚',
};

static SHORT_FORMS: phf::Set<&str> = phf_set! {
    "L⠁L⠃", "L⠁L⠃L⠧", "L⠁L⠉", "L⠁L⠉L⠗", "L⠁L⠋",
    "L⠁L⠋L⠝", "L⠁L⠋L⠺", "L⠁L⠛", "L⠁L⠛L⠌", "L⠁L⠇",
     "L⠁L⠇L⠍", "L⠁L⠇L⠗", "L⠁L⠇L⠞", "L⠁L⠇L⠹", "L⠁L⠇L⠺",
     "L⠃L⠇", "L⠃L⠗L⠇", "L⠉L⠙", "L⠙L⠉L⠇", "L⠙L⠉L⠇L⠛",
     "L⠙L⠉L⠧", "L⠙L⠉L⠧L⠛", "L⠑L⠊", "L⠋L⠗", "L⠋L⠌", "L⠛L⠙",
     "L⠛L⠗L⠞", "L⠓L⠍", "L⠓L⠍L⠋", "L⠓L⠻L⠋", "L⠊L⠍L⠍", "L⠇L⠇", "L⠇L⠗",
     "L⠍L⠽L⠋", "L⠍L⠡", "L⠍L⠌", "L⠝L⠑L⠉", "L⠝L⠑L⠊", "L⠏L⠙",
     "L⠏L⠻L⠉L⠧", "L⠏L⠻L⠉L⠧L⠛", "L⠏L⠻L⠓", "L⠟L⠅", "L⠗L⠉L⠧",
     "L⠗L⠉L⠧L⠛", "L⠗L⠚L⠉", "L⠗L⠚L⠉L⠛", "L⠎L⠙", "L⠎L⠡", "L⠞L⠙",
     "L⠞L⠛L⠗", "L⠞L⠍", "L⠞L⠝", "L⠭L⠋", "L⠭L⠎", "L⠽L⠗", "L⠽L⠗L⠋",
     "L⠽L⠗L⠧L⠎", "L⠮L⠍L⠧L⠎", "L⠡L⠝", "L⠩L⠙", "L⠹L⠽L⠋", "L⠳L⠗L⠧L⠎",
     "L⠺L⠙", "L⠆L⠉", "L⠆L⠋", "L⠆L⠓", "L⠆L⠇", "L⠆L⠝", "L⠆L⠎", "L⠆L⠞",
     "L⠆L⠽", "L⠒L⠉L⠧", "L⠒L⠉L⠧L⠛", "L⠐L⠕L⠋"
};

static LETTER_PREFIXES: phf::Set<char> = phf_set! {
    'B', 'I', '𝔹', 'S', 'T', 'D', 'C', '𝐶', '𝑐',
};

lazy_static! {
    // Trim braille spaces before and after braille indicators
    // In order: fraction, /, cancellation, letter, baseline
    // Note: fraction over is not listed due to example 42(4) which shows a space before the "/"
    // static ref REMOVE_SPACE_BEFORE_BRAILLE_INDICATORS: Regex = 
    //     Regex::new(r"(⠄⠄⠄|⠤⠤⠤)W+([⠼⠸⠪])").unwrap();
    static ref REPLACE_INDICATORS: Regex =Regex::new(r"([1𝟙SB𝔹TIREDGVHP𝐶𝑐CLMNW𝐖swe,.-—―#ocb])").unwrap();  
    static ref COLLAPSE_SPACES: Regex = Regex::new(r"⠀⠀+").unwrap();
}

fn is_short_form(chars: &[char]) -> bool {
    let chars_as_string = chars.iter().map(|ch| ch.to_string()).collect::<String>();
    return SHORT_FORMS.contains(&chars_as_string);
}

fn ueb_cleanup(pref_manager: Ref<PreferenceManager>, raw_braille: String) -> String {
    debug!("ueb_cleanup: start={}", raw_braille);
    let result = typeface_to_word_mode(&raw_braille);
    let result = capitals_to_word_mode(&result);

    let use_only_grade1 = pref_manager.pref_to_string("UEB_START_MODE").as_str() == "Grade1";
    
    // '𝐖' is a hard break -- basically, it separates exprs
    let mut result = result.split('𝐖')
                        .map(|str| pick_start_mode(str, use_only_grade1) + "W")
                        .collect::<String>();
    result.pop();   // we added a 'W' at the end that needs to be removed.

    let result = result.replace("tW", "W");

    // these typeforms need to get pulled from user-prefs as they are transcriber-defined
    let double_struck = pref_manager.pref_to_string("UEB_DoubleStruck");
    let sans_serif = pref_manager.pref_to_string("UEB_SansSerif");
    let fraktur = pref_manager.pref_to_string("UEB_Fraktur");
    let greek_variant = pref_manager.pref_to_string("Vietnam_GreekVariant");

    let result = REPLACE_INDICATORS.replace_all(&result, |cap: &Captures| {
        let matched_char = &cap[0];
        match matched_char {
            "𝔹" => &double_struck,
            "S" => &sans_serif,
            "D" => &fraktur,
            "V" => &greek_variant,
            _ => match UEB_INDICATOR_REPLACEMENTS.get(matched_char) {
                None => {error!("REPLACE_INDICATORS and UEB_INDICATOR_REPLACEMENTS are not in sync: missing '{}'", matched_char); ""},
                Some(&ch) => ch,
            },
        }
    });

    // Remove unicode blanks at start and end -- do this after the substitutions because ',' introduces spaces
    // let result = result.trim_start_matches('⠀').trim_end_matches('⠀');
    let result = COLLAPSE_SPACES.replace_all(&result, "⠀");
   
    return result.to_string();

    fn pick_start_mode(raw_braille: &str, use_only_grade1: bool) -> String {
        // Need to decide what the start mode should be
        // From http://www.brailleauthority.org/ueb/ueb_math_guidance/final_for_posting_ueb_math_guidance_may_2019_102419.pdf
        //   Unless a math expression can be correctly represented with only a grade 1 symbol indicator in the first three cells
        //   or before a single letter standing alone anywhere in the expression,
        //   begin the expression with a grade 1 word indicator (or a passage indicator if the expression includes spaces)
        // Apparently "only a grade 1 symbol..." means at most one grade 1 symbol based on some examples (GTM 6.4, example 4)
        // debug!("before determining mode:  '{}'", raw_braille);
        if use_only_grade1 {
            return remove_unneeded_mode_changes(raw_braille, UEB_Mode::Grade1, UEB_Duration::Passage); 
        }
        let grade2 = remove_unneeded_mode_changes(raw_braille, UEB_Mode::Grade2, UEB_Duration::Symbol);
        debug!("Symbol mode:  '{}'", grade2);

        if is_grade2_string_ok(&grade2) {
            return grade2;
        } else {
            let grade1_word = remove_unneeded_mode_changes(raw_braille, UEB_Mode::Grade1, UEB_Duration::Word);
            debug!("Word mode:    '{}'", grade1_word);
            
            // BANA says use g1 word mode if spaces are present, but that's not what their examples do
            // A conversation with Ms. DeAndrea from BANA said that they mean use passage mode if ≥3 "segments" (≥2 blanks)
            // However, it is pointless to go into passage mode if the internal string is the same as word mode
            let mut grade1_passage = "".to_string();
            let mut n_blanks = 0;
            if grade1_word.chars().any(|ch| {
                if ch == 'W' {
                    n_blanks += 1;
                }
                n_blanks == 2
            }) {
                grade1_passage = remove_unneeded_mode_changes(raw_braille, UEB_Mode::Grade1, UEB_Duration::Passage);
                // debug!("Passage mode: '{}'", &grade1_passage);
            }
            if grade1_passage.is_empty() || grade1_passage == grade1_word {
                return "⠰⠰".to_string() + &grade1_word;
            } else {
                return "⠰⠰⠰".to_string() + &grade1_passage + "⠰⠄";
            }
        }

        /// Return true if the BANA guidelines say it is ok to start with grade 2
        fn is_grade2_string_ok(grade2_braille: &str) -> bool {
            // BANA says use grade 2 if there is not more than one grade one symbol or single letter standing alone.
            // The exact quote from their guidance:
            //    Unless a math expression can be correctly represented with only a grade 1 symbol indicator in the first three cells
            //    or before a single letter standing alone anywhere in the expression,
            //    begin the expression with a grade 1 word indicator
            // Note: I modified this slightly to exclude the cap indicator in the count. That allows three more ICEB rule to pass and seems
            //    like it is a reasonable thing to do.

            // Because of the 'L's which go away, we have to put a little more work into finding the first three chars
            let chars = grade2_braille.chars().collect::<Vec<char>>();
            let mut n_real_chars = 0;  // actually number of chars
            let mut found_g1 = false;
            let mut i = 0;      // chars starts on the 4th char
            while i < chars.len() {
                let ch = chars[i];
                if ch == '1' && !is_forced_grade1(&chars, i) {
                    if found_g1 {
                        return false;
                    }
                    found_g1 = true;
                } else if !"𝐶CLobc".contains(ch) {
                    if n_real_chars == 2 {
                        i += 1;
                        break;      // this is the third real char
                    };
                    n_real_chars += 1;
                }
                i += 1
            }

            // if we find another g1 that isn't forced and isn't standing alone, we are done
            // we only allow one standing alone example -- not sure if BANA guidance has this limit, but GTM 11_5_5_3 seems better with it
            let mut is_standing_alone_already_encountered = false;
            while i < chars.len() {
                let ch = chars[i];
                if ch == '1' && !is_forced_grade1(&chars, i) {
                    if !is_single_letter_on_right(&chars, i) || is_standing_alone_already_encountered {
                        return false;
                    }
                    is_standing_alone_already_encountered = true; 
                }
                i += 1;
            }
            return true;
        }

        /// Return true if the sequence of chars forces a '1' at the `i`th position
        /// Note: `chars[i]` should be '1'
        fn is_forced_grade1(chars: &[char], i: usize) -> bool {
            // A '1' is forced if 'a-j' follows a digit
            assert_eq!(chars[i], '1', "'is_forced_grade1' didn't start with '1'");
            // check that a-j follows the '1' -- we have '1Lx' where 'x' is the letter to check
            if i+2 < chars.len() && LETTER_NUMBERS.contains(&unhighlight(chars[i+2])) {
                // check for a number before the '1'
                // this will be 'N' followed by LETTER_NUMBERS or the number ".", ",", or " "
                for j in (0..i).rev() {
                    let ch = chars[j];
                    if !(LETTER_NUMBERS.contains(&unhighlight(ch)) || ".,W𝐖".contains(ch)) {
                        return ch == 'N'
                    }
                }
            }
            return false;
        }

        fn is_single_letter_on_right(chars: &[char], i: usize) -> bool {
            static SKIP_CHARS: phf::Set<char> = phf_set! {
                'B', 'I', '𝔹', 'S', 'T', 'D', 'C', '𝐶', 's', 'w'   // indicators
            };

            // find the first char (if any)
            let mut count = 0;      // how many letters
            let mut i = i+1;
            while i < chars.len() {
                let ch = chars[i];
                if !SKIP_CHARS.contains(&ch) {
                    if ch == 'L' {
                        if count == 1 {
                            return false;   // found a second letter in the sequence
                        }
                        count += 1;
                    } else {
                        return count==1;
                    }
                    i += 2;   // eat 'L' and actual letter
                } else {
                    i += 1;
                }
            }
            return true;
        }
    }
}

fn typeface_to_word_mode(braille: &str) -> String {
    lazy_static! {
        static ref HAS_TYPEFACE: Regex = Regex::new("[BI𝔹STD]").unwrap();
    }
    // debug!("before typeface fix:  '{}'", braille);

    let mut result = "".to_string();
    let chars = braille.chars().collect::<Vec<char>>();
    let mut word_mode = Vec::with_capacity(5);
    let mut word_mode_end = Vec::with_capacity(5);
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if HAS_TYPEFACE.is_match(ch.to_string().as_str()) {
            let i_next_char_target = find_next_char(&chars[i+1..], ch);
            if word_mode.contains(&ch) {
                if i_next_char_target.is_none() {
                    word_mode.retain(|&item| item!=ch);  // drop the char since word mode is done
                    word_mode_end.push(ch);   // add the char to signal to add end sequence
                }
            } else {
                result.push(ch);
                if i_next_char_target.is_some() {
                    result.push('w');     // typeface word indicator
                    word_mode.push(ch);      // starting word mode for this char
                } else {
                    result.push('s');     // typeface single char indicator
                }
            }
            i += 1; // eat "B", etc
        } else if ch == 'L' || ch == 'N' {
            result.push(chars[i]);
            result.push(chars[i+1]);
            if !word_mode_end.is_empty() && i+2 < chars.len() && !(chars[i+2] == 'W'|| chars[i+2] == '𝐖') {
                // add terminator unless word sequence is terminated by end of string or whitespace
                for &ch in &word_mode_end {
                    result.push(ch);
                    result.push('e');
                };
                word_mode_end.clear();
            }
            i += 2; // eat Ll/Nd
        } else {
            result.push(ch);
            i += 1;
        }
    }
    return result;

}

fn capitals_to_word_mode(braille: &str) -> String {
    use std::iter::FromIterator;
    // debug!("before capitals fix:  '{}'", braille);

    let mut result = "".to_string();
    let chars = braille.chars().collect::<Vec<char>>();
    let mut is_word_mode = false;
    let mut i = 0;
    // look for a sequence of CLxCLy... and create CCLxLy...
    while i < chars.len() {
        let ch = chars[i];
        if ch == 'C' {
            // '𝑐' should only occur after a 'C', so we don't have top-level check for it
            let mut next_non_cap = i+1;
            while let Some(i_next) = find_next_char(&chars[next_non_cap..], '𝑐') {
                next_non_cap += i_next + 1; // C/𝑐, L, letter
            }
            if find_next_char(&chars[next_non_cap..], 'C').is_some() { // next letter sequence "C..."
                if is_next_char_start_of_section_12_modifier(&chars[next_non_cap+1..]) {
                    // to me this is tricky -- section 12 modifiers apply to the previous item
                    // the last clause of the "item" def is the previous "individual symbol" which ICEB 2.1 say is:
                    //   braille sign: one or more consecutive braille characters comprising a unit,
                    //     consisting of a root on its own or a root preceded by one or more
                    //     prefixes (also referred to as braille symbol)
                    // this means the capital indicator needs to be stated and can't be part of a word or passage
                    is_word_mode = false;
                    result.push_str(String::from_iter(&chars[i..next_non_cap]).as_str());
                    i = next_non_cap;
                    continue;
                }
                if is_word_mode {
                    i += 1;     // skip the 'C'
                } else {
                    // start word mode -- need an extra 'C'
                    result.push('C');
                    is_word_mode = true;
                }
            } else if is_word_mode {
                i += 1;         // skip the 'C'
            }
            if chars[next_non_cap] == 'G' {
                // Greek letters are a bit exceptional in that the pattern is "CGLx" -- bump 'i'
                next_non_cap += 1;
            }
            if chars[next_non_cap] != 'L' {
                error!("capitals_to_word_mode: internal error: didn't find L after C in '{}'.",
                       chars[i..next_non_cap+2].iter().collect::<String>().as_str());
            }
            let i_braille_char = next_non_cap + 2;
            result.push_str(String::from_iter(&chars[i..i_braille_char]).as_str());
            i = i_braille_char;
        } else if ch == 'L' {       // must be lowercase -- uppercase consumed above
            // assert!(LETTERS.contains(&unhighlight(chars[i+1]))); not true for other alphabets
            if is_word_mode {
                result.push('e');       // terminate Word mode (letter after caps)
                is_word_mode = false;
            }
            result.push('L');
            result.push(chars[i+1]);
            i += 2; // eat L, letter
        } else {
            is_word_mode = false;   // non-letters terminate cap word mode
            result.push(ch);
            i += 1;
        }
    }
    return result;

    fn is_next_char_start_of_section_12_modifier(chars: &[char]) -> bool {
        // first find the L and eat the char so that we are at the potential start of where the target lies
        let chars_len = chars.len();
        let mut i_cap = 0;
        while chars[i_cap] != 'C' {     // we know 'C' is in the string, so no need to check for exceeding chars_len
            i_cap += 1;
        }
        for i_end in i_cap+1..chars_len {
            if chars[i_end] == 'L' {
                // skip the next char to get to the real start, and then look for the modifier string or next L/N
                // debug!("   after L '{}'", chars[i_end+2..].iter().collect::<String>());
                for i in i_end+2..chars_len {
                    let ch = chars[i]; 
                    if ch == '1' {
                        // Fix: there's probably a much better way to check if we have a match against one of "⠱", "⠘⠱", "⠘⠲", "⠸⠱", "⠐⠱ ", "⠨⠸⠱"
                        if chars[i+1] == '⠱' {
                            return true;
                        } else if i+2 < chars_len {
                            let mut str = chars[i+1].to_string();
                            str.push(chars[i+2]);
                            if str == "⠘⠱" || str == "⠘⠲" || str == "⠸⠱" || str == "⠐⠱" {
                                return true;
                            } else if i+3 < chars_len {
                                str.push(chars[i+3]);
                                return str == "⠨⠸⠱";
                            }
                            return false;
                        }
                    }
                    if ch == 'L' || ch == 'N' || !LETTER_PREFIXES.contains(&ch) {
                        return false;
                    }
                }
            }
        }
        return false;
    }    
}

fn find_next_char(chars: &[char], target: char) -> Option<usize> {        
    // first find the L or N and eat the char so that we are at the potential start of where the target lies
    // debug!("Looking for '{}' in '{}'", target, chars.iter().collect::<String>());
    for i_end in 0..chars.len() {
        if chars[i_end] == 'L' || chars[i_end] == 'N' {
            // skip the next char to get to the real start, and then look for the target
            // stop when L/N signals past potential target or we hit some non L/N char (actual braille)
            // debug!("   after L/N '{}'", chars[i_end+2..].iter().collect::<String>());
            for (i, &ch) in chars.iter().enumerate().skip(i_end+2) {
                if ch == 'L' || ch == 'N' || !LETTER_PREFIXES.contains(&ch) {
                    return None;
                } else if ch == target {
                    // debug!("   found target");
                    return Some(i);
                }
            }
        }
    }
    return None;
}

#[allow(non_camel_case_types)]
#[derive(Debug, PartialEq, Copy, Clone)]
enum UEB_Mode {
    Numeric,        // also includes Grade1
    Grade1,
    Grade2,
}

#[allow(non_camel_case_types)]
#[derive(Debug, PartialEq, Copy, Clone)]
enum UEB_Duration {
    // Standing alone: A braille symbol that is standing alone may have a contracted (grade 2) meaning.
    // A letter or unbroken sequence of letters is “standing alone” if the symbols before and after the letter or
    //   sequence are spaces, hyphens, dashes or any combination thereof, including some common punctuation.
    // Item: An “item” is defined as the next symbol or one of seven groupings listed in Rules of Unified English Braille, §11.4.1.
    Symbol,

    // The grade 1 word indicator sets grade 1 mode for the next word or symbol sequence.
    // A symbol sequence in UEB is defined as an unbroken string of braille signs,
    //   whether alphabetic or non-alphabetic, preceded and followed by a space.
    Word,
    Passage,
}

// used to determine standing alone (on left side)
static LEFT_INTERVENING_CHARS: phf::Set<char> = phf_set! {  // see RUEB 2.6.2
    'B', 'I', '𝔹', 'S', 'T', 'D', 'C', '𝐶', 's', 'w',     // indicators
    // opening chars have prefix 'o', so not in set ['(', '{', '[', '"', '\'', '“', '‘', '«'] 
};

fn remove_unneeded_mode_changes(raw_braille: &str, start_mode: UEB_Mode, start_duration: UEB_Duration) -> String {

    // FIX: need to be smarter about moving on wrt to typeforms/typefaces, caps, bold/italic. [maybe just let them loop through the default?]
    let mut mode = start_mode;
    let mut duration = start_duration;
    let mut start_g2_letter = None;    // used for start of contraction checks
    let mut i_g2_start = None;  // set to 'i' when entering G2 mode; None in other modes. '1' indicator goes here if standing alone
    let mut cap_word_mode = false;     // only set to true in G2 to prevent contractions
    let mut result = String::default();
    let chars = raw_braille.chars().collect::<Vec<char>>();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        match mode {
            UEB_Mode::Numeric => {
                // Numeric Mode: (from https://uebmath.aphtech.org/lesson1.0 and lesson4.0)
                // Symbols that can appear within numeric mode include the ten digits, comma, period, simple fraction line,
                // line continuation indicator, and numeric space digit symbols.
                // A space or any other symbol not listed here terminates numeric mode.
                // Numeric mode is also terminated by the "!" -- used after a script
                //
                // The numeric indicator also turns on grade 1 mode.
                // When grade 1 mode is set by the numeric indicator,
                //   grade 1 indicators are not used unless a single lower-case letter a-j immediately follows a digit.
                // Grade 1 mode when set by the numeric indicator is terminated by a space, hyphen, dash, or a grade 1 indicator.
                i_g2_start = None;
                // debug!("Numeric: ch={}, duration: {:?}", ch, duration);
                match ch {
                    'L' => {
                        // terminate numeric mode -- duration doesn't change
                        // let the default case handle pushing on the chars for the letter
                        if LETTER_NUMBERS.contains(&unhighlight(chars[i+1])) {
                            result.push('1');   // need to distinguish a-j from a digit
                        }
                        result.push(ch);
                        i += 1;
                        mode = UEB_Mode::Grade1;
                        // duration remains Word
                    },
                    '1' | '𝟙' => {
                        // numeric mode implies grade 1, so don't output indicator;
                        i += 1;
                        mode = UEB_Mode::Grade1;
                        if start_duration == UEB_Duration::Passage {
                            duration = UEB_Duration::Passage;      // otherwise it remains at Word
                        }
                    },
                    '#' => {
                        // terminate numeric mode -- duration doesn't change
                        i += 1;
                        if i+1 < chars.len() && chars[i] == 'L' && LETTER_NUMBERS.contains(&unhighlight(chars[i+1])) {
                            // special case where the script was numeric and a letter follows, so need to put out G1 indicator
                            result.push('1');
                            // the G1 case should work with 'L' now
                        }
                        mode = UEB_Mode::Grade1;
                    },
                    'N' => {
                        // stay in the same mode (includes numeric "," and "." space) -- don't let default get these chars
                        result.push(chars[i+1]);
                        i += 2;
                    },
                    _ => {
                        // moving out of numeric mode
                        result.push(ch);
                        i += 1;
                        mode = if "W𝐖-—―".contains(ch) {start_mode} else {UEB_Mode::Grade1};     // space, hyphen, dash(short & long) RUEB 6.5.1
                        if mode == UEB_Mode::Grade2 {
                            start_g2_letter = None;        // will be set to real letter
                        }
                    },
                }
            },
            UEB_Mode::Grade1 => {
                // Grade 1 Mode:
                // The numeric indicator also sets grade 1 mode.
                // Grade 1 mode, when initiated by the numeric indicator, is terminated by a space, hyphen, dash or grade 1 terminator.
                // Grade 1 mode is also set by grade 1 indicators.
                i_g2_start = None;
                // debug!("Grade 1: ch={}, duration: {:?}", ch, duration);
                match ch {
                    'L' => {
                        // note: be aware of '#' case for Numeric because '1' might already be generated
                        // let prev_ch = if i > 1 {chars[i-1]} else {'1'};   // '1' -- anything beside ',' or '.'
                        // if duration == UEB_Duration::Symbol || 
                        //     ( ",. ".contains(prev_ch) && LETTER_NUMBERS.contains(&unhighlight(chars[i+1])) ) {
                        //     result.push('1');        // need to retain grade 1 indicator (RUEB 6.5.2)
                        // }
                        // let the default case handle pushing on the chars for the letter
                        result.push(ch);
                        i += 1;
                    },
                    '1' | '𝟙' => {
                        if ch == '𝟙' {
                            duration = UEB_Duration::Word;
                        }
                        // nothing to do -- let the default case handle the following chars
                        i += 1;
                    },
                    'N' => {
                        result.push(ch);
                        result.push(chars[i+1]);
                        i += 2;
                        mode = UEB_Mode::Numeric;
                        duration = UEB_Duration::Word;
                    },
                    'W' | '𝐖' => {
                        // this terminates a word mode if there was one
                        result.push(ch);
                        i += 1;
                        if start_duration != UEB_Duration::Passage {
                            duration = UEB_Duration::Symbol;
                            mode = UEB_Mode::Grade2;
                        }
                    },
                    _ => {
                        result.push(ch);
                        i += 1;
                        if duration == UEB_Duration::Symbol && !LETTER_PREFIXES.contains(&ch) {
                            mode = start_mode;
                        }
                    }
                }
                if mode == UEB_Mode::Grade2 {
                    start_g2_letter = None;        // will be set to real letter
                }

            },
            UEB_Mode::Grade2 => {
                // note: if we ended up using a '1', it only extends to the next char, which is also dealt with, so mode doesn't change
               if i_g2_start.is_none() {
                   i_g2_start = Some(i);
                   cap_word_mode = false;
               }
                // debug!("Grade 2: ch={}, duration: {:?}", ch, duration);
                match ch {
                    'L' => {
                        if start_g2_letter.is_none() {
                            start_g2_letter = Some(i);
                        }
                        let (is_alone, right_matched_chars, n_letters) = stands_alone(&chars, i);
                        // GTM 1.2.1 says we only need to use G1 for single letters or sequences that are a shortform (e.g, "ab")
                        if is_alone && (n_letters == 1 || is_short_form(&right_matched_chars[..2*n_letters])) {
                            // debug!("  is_alone -- pushing '1'");
                            result.push('1');
                            mode = UEB_Mode::Grade1;
                        }
                        // debug!("  pushing {:?}", right_matched_chars);
                        right_matched_chars.iter().for_each(|&ch| result.push(ch));
                        i += right_matched_chars.len();
                    },
                    'C' => {
                        // Want 'C' before 'L'; Could be CC for word cap -- if so, eat it and move on
                        // Note: guaranteed that there is a char after the 'C', so chars[i+1] is safe
                        if chars[i+1] == 'C' {
                            cap_word_mode = true;
                            i += 1;
                        } else {
                            let is_greek = chars[i+1] == 'G';
                            let (is_alone, right_matched_chars, n_letters) = stands_alone(&chars, if is_greek {i+2} else {i+1});
                            // GTM 1.2.1 says we only need to use G1 for single letters or sequences that are a shortform (e.g, "ab")
                            if is_alone && (n_letters == 1 || is_short_form(&right_matched_chars[..2*n_letters])) {
                                // debug!("  is_alone -- pushing '1'");
                                result.push('1');
                                mode = UEB_Mode::Grade1;
                            }
                            if cap_word_mode {
                                result.push('C');   // first 'C' if cap word
                            }
                            result.push('C');
                            if is_greek {
                                result.push('G');
                                i += 1;
                            }
                            start_g2_letter = Some(i);
                            // debug!("  pushing 'C' + {:?}", right_matched_chars);
                            right_matched_chars.iter().for_each(|&ch| result.push(ch));
                            i += 1 + right_matched_chars.len();
                        }
                    },
                    '1' | '𝟙' => {
                        result.push(ch);
                        i += 1;
                        mode = UEB_Mode::Grade1;
                        duration = if ch=='1' {UEB_Duration::Symbol} else {UEB_Duration::Word};
                    },
                    'N' => {
                        result.push(ch);
                        result.push(chars[i+1]);
                        i += 2;
                        mode = UEB_Mode::Numeric;
                        duration = UEB_Duration::Word;
                    },
                    _ => {
                        if let Some(start) = start_g2_letter {
                            if !cap_word_mode {
                                result = handle_contractions(&chars[start..i], result);
                            }
                            cap_word_mode = false;
                            start_g2_letter = None;     // not start of char sequence
                        }
                        result.push(ch);
                        i += 1;
                        if !LEFT_INTERVENING_CHARS.contains(&ch) {
                            cap_word_mode = false;
                            i_g2_start = Some(i);
                        }

                    }
                }
                if mode != UEB_Mode::Grade2 && !cap_word_mode {
                    if let Some(start) = start_g2_letter {
                        result = handle_contractions(&chars[start..i], result);
                        start_g2_letter = None;     // not start of char sequence
                    }
                }
            },
        }
    }
    if mode == UEB_Mode::Grade2 {
        if let Some(start) = start_g2_letter {
            result = handle_contractions(&chars[start..i], result);
        }
    }

    return result;
}

/// Returns a tuple:
///   true if the ith char "stands alone" (UEB 2.6)
///   the chars on the right that are part of the standing alone sequence
///   the number of letters in that sequence
/// This basically means a letter sequence surrounded by white space with some potentially intervening chars
/// The intervening chars can be typeform/cap indicators, along with various forms of punctuation
/// The ith char should be an "L"
/// This assumes that there is whitespace before and after the character string
fn stands_alone(chars: &[char], i: usize) -> (bool, &[char], usize) {
    // scan backward and check the conditions for "standing-alone"
    // we scan forward and check the conditions for "standing-alone"
    assert_eq!(chars[i], 'L', "'stands_alone' starts with non 'L'");
    // debug!("stands_alone: i={}, chars: {:?}", i, chars);
    if !left_side_stands_alone(&chars[0..i]) {
        return (false, &chars[i..i+2], 0);
    }

    let (mut is_alone, n_letters, n_right_matched) = right_side_stands_alone(&chars[i+2..]);
    // debug!("left is alone, right is alone: {}, : n_letters={}, n_right_matched={}", is_alone, n_letters, n_right_matched);

    if is_alone && n_letters == 1 {
        let ch = chars[i+1];
        if ch=='⠁' || ch=='⠊' || ch=='⠕' {      // a, i, o
            is_alone = false;
        }
    }
    return (is_alone, &chars[i..i+2+n_right_matched], n_letters);

    /// chars before before 'L'
    fn left_side_stands_alone(chars: &[char]) -> bool {
        // scan backwards to skip letters and intervening chars
        // once we hit an intervening char, only intervening chars are allowed if standing alone
        let mut intervening_chars_mode = false; // true when we are on the final stretch
        let mut i = chars.len();
        while i > 0 {
            i -= 1;
            let ch = chars[i];
            let prev_ch = if i > 0 {chars[i-1]} else {' '};  // ' ' is a char not in input
            // debug!("  left alone: prev/ch {}/{}", prev_ch, ch);
            if (!intervening_chars_mode && prev_ch == 'L') ||
               (prev_ch == 'o' || prev_ch == 'b') {
                intervening_chars_mode = true;
                i -= 1;       // ignore 'Lx' and also ignore 'ox'
            } else if LEFT_INTERVENING_CHARS.contains(&ch) {
                intervening_chars_mode = true;
            } else {
                return "W𝐖-—―".contains(ch);
            }
        }

        return true;
    }

    // chars after character we are testing
    fn right_side_stands_alone(chars: &[char]) -> (bool, usize, usize) {
        // see RUEB 2.6.3
        static RIGHT_INTERVENING_CHARS: phf::Set<char> = phf_set! {
            'B', 'I', '𝔹', 'S', 'T', 'D', 'C', '𝐶', 's', 'w', 'e',   // indicators
            // ')', '}', ']', '\"', '\'', '”', '’', '»',      // closing chars
            // ',', ';', ':', '.', '…', '!', '?'              // punctuation           
        };
        // scan forward to skip letters and intervening chars
        // once we hit an intervening char, only intervening chars are allowed if standing alone ('c' and 'b' are part of them)
        let mut intervening_chars_mode = false; // true when we are on the final stretch
        let mut i = 0;
        let mut n_letters = 1;      // we have skipped the first letter
        while i < chars.len() {
            let ch = chars[i];
            // debug!("  right alone: ch/next {}/{}", ch, if i+1<chars.len() {chars[i+1]} else {' '});
            if !intervening_chars_mode && ch == 'L' {
                n_letters += 1;
                i += 1;       // ignore 'Lx' and also ignore 'ox'
            } else if ch == 'c' || ch == 'b' {
                i += 1;       // ignore 'Lx' and also ignore 'ox'
            } else if RIGHT_INTERVENING_CHARS.contains(&ch) {  
                intervening_chars_mode = true;
            } else {
                return if "W𝐖-—―".contains(ch) {(true, n_letters, i)} else {(false, n_letters, i)};
            }
            i += 1;
        }

        return (true, n_letters, chars.len());
    }
}

/// Return a modified result if chars can be contracted.
/// Otherwise, the original string is returned
fn handle_contractions(chars: &[char], mut result: String) -> String {
    struct Replacement {
        pattern: &'static str,
        replacement: &'static str
    }

    // It would be much better from an extensibility point of view to read the table in from a file
    // FIX: this would be much easier to read/maintain if ASCII braille were used
    // FIX:   (without the "L"s) and the CONTRACTIONS table built as a lazy static
    static CONTRACTIONS: &[Replacement] = &[
        Replacement{ pattern: "L⠁L⠝L⠙", replacement: "L⠯" },           // and
        Replacement{ pattern: "L⠋L⠕L⠗", replacement: "L⠿" },           // for
        Replacement{ pattern: "L⠕L⠋", replacement: "L⠷" },             // of
        Replacement{ pattern: "L⠞L⠓L⠑", replacement: "L⠮" },           // the
        Replacement{ pattern: "L⠺L⠊L⠞L⠓", replacement: "L⠾" },         // with
        Replacement{ pattern: "L⠉L⠓", replacement: "L⠡" },              // ch
        Replacement{ pattern: "L⠊L⠝", replacement: "L⠔" },              // in

        // cc -- don't match if after/before a cap letter -- no/can't use negative pattern (?!...) in regex package
        // figure this out -- also applies to ea, bb, ff, and gg (not that they matter)
        // cc may be important for "arccos", but RUEB doesn't apply it to "arccosine", so maybe not
        // Replacement{ pattern: "L⠉L⠉", replacement: "L⠒" },              // cc -- don't match if after/before a cap letter
        
        
        Replacement{ pattern: "L⠎L⠓", replacement: "L⠩" },              // sh
        Replacement{ pattern: "L⠁L⠗", replacement: "L⠜" },              // ar
        Replacement{ pattern: "L⠑L⠗", replacement: "L⠻" },              // er
        Replacement{ pattern: "(?P<s>L.)L⠍L⠑L⠝L⠞", replacement: "${s}L⠰L⠞" }, // ment
        Replacement{ pattern: "(?P<s>L.)L⠞L⠊L⠕L⠝", replacement: "${s}L⠰L⠝" } ,// tion
        Replacement{ pattern: "(?P<s>L.)L⠑L⠁(?P<e>L.)", replacement: "${s}L⠂${e}" },  // ea
    ];

    lazy_static! {
        static ref CONTRACTION_PATTERNS: RegexSet = init_patterns(CONTRACTIONS);
        static ref CONTRACTION_REGEX: Vec<Regex> = init_regex(CONTRACTIONS);
    }

    let mut chars_as_str = chars.iter().collect::<String>();
    // debug!("  handle_contractions: examine '{}'", &chars_as_str);
    let matches = CONTRACTION_PATTERNS.matches(&chars_as_str);
    for i in matches.iter() {
        let element = &CONTRACTIONS[i];
        // debug!("  replacing '{}' with '{}' in '{}'", element.pattern, element.replacement, &chars_as_str);
        result.truncate(result.len() - chars_as_str.len());
        chars_as_str = CONTRACTION_REGEX[i].replace_all(&chars_as_str, element.replacement).to_string();
        result.push_str(&chars_as_str);
        // debug!("  result after replace '{}'", result);
    }
    return result;



    fn init_patterns(contractions: &[Replacement]) -> RegexSet {
        let mut vec = Vec::with_capacity(contractions.len());
        for contraction in contractions {
            vec.push(contraction.pattern);
        }
        return RegexSet::new(&vec).unwrap();
    }

    fn init_regex(contractions: &[Replacement]) -> Vec<Regex> {
        let mut vec = Vec::with_capacity(contractions.len());
        for contraction in contractions {
            vec.push(Regex::new(contraction.pattern).unwrap());
        }
        return vec;
    }
}




static VIETNAM_INDICATOR_REPLACEMENTS: phf::Map<&str, &str> = phf_map! {
    "S" => "XXX",    // sans-serif -- from prefs
    "B" => "⠘",     // bold
    "𝔹" => "XXX",     // blackboard -- from prefs
    "T" => "⠈",     // script
    "I" => "⠨",     // italic
    "R" => "",      // roman
    // "E" => "⠰",     // English
    "1" => "⠠",     // Grade 1 symbol
    "L" => "",     // Letter left in to assist in locating letters
    "D" => "XXX",     // German (Deutsche) -- from prefs
    "G" => "⠰",     // Greek
    "V" => "XXX",    // Greek Variants
    // "H" => "⠠⠠",    // Hebrew
    // "U" => "⠈⠈",    // Russian
    "C" => "⠨",      // capital
    "𝑐" => "",       // second or latter braille cell of a capital letter
    "𝐶" => "⠨",      // capital that never should get word indicator (from chemical element)
    "N" => "⠼",     // number indicator
    "t" => "⠱",     // shape terminator
    "W" => "⠀",     // whitespace"
    "𝐖"=> "⠀",     // whitespace
    "s" => "⠆",     // typeface single char indicator
    "w" => "",     // typeface word indicator
    "e" => "",     // typeface & capital terminator 
    "o" => "",       // flag that what follows is an open indicator (used for standing alone rule)
    "c" => "",     // flag that what follows is an close indicator (used for standing alone rule)
    "b" => "",       // flag that what follows is an open or close indicator (used for standing alone rule)
    "," => "⠂",     // comma
    "." => "⠲",     // period
    "-" => "-",     // hyphen
    "—" => "⠠⠤",   // normal dash (2014) -- assume all normal dashes are unified here [RUEB appendix 3]
    "―" => "⠐⠠⠤",  // long dash (2015) -- assume all long dashes are unified here [RUEB appendix 3]
    "#" => "",      // signals end of script

};

fn vietnam_cleanup(pref_manager: Ref<PreferenceManager>, raw_braille: String) -> String {
    debug!("vietnam_cleanup: start={}", raw_braille);
    let result = typeface_to_word_mode(&raw_braille);
    let result = capitals_to_word_mode(&result);

    let result = result.replace("tW", "W");
    let result = result.replace("CG", "⠸");    // capital Greek letters are problematic in Vietnam braille
    let result = result.replace("CC", "⠸");    // capital word more is the same as capital Greek letters
    debug!("   after typeface/caps={}", &result);

    // these typeforms need to get pulled from user-prefs as they are transcriber-defined
    let double_struck = pref_manager.pref_to_string("Vietnam_DoubleStruck");
    let sans_serif = pref_manager.pref_to_string("Vietnam_SansSerif");
    let fraktur = pref_manager.pref_to_string("Vietnam_Fraktur");
    let greek_variant = pref_manager.pref_to_string("Vietnam_GreekVariant");

    // This reuses the code just for getting rid of unnecessary "L"s and "N"s
    let result = remove_unneeded_mode_changes(&result, UEB_Mode::Grade1, UEB_Duration::Passage);


    let result = REPLACE_INDICATORS.replace_all(&result, |cap: &Captures| {
        let matched_char = &cap[0];
        match matched_char {
            "𝔹" => &double_struck,
            "S" => &sans_serif,
            "D" => &fraktur,
            "V" => &greek_variant,
            _ => match VIETNAM_INDICATOR_REPLACEMENTS.get(matched_char) {
                None => {error!("REPLACE_INDICATORS and VIETNAM_INDICATOR_REPLACEMENTS are not in sync: missing '{}'", matched_char); ""},
                Some(&ch) => ch,
            },
        }
    });

    // Remove unicode blanks at start and end -- do this after the substitutions because ',' introduces spaces
    // let result = result.trim_start_matches('⠀').trim_end_matches('⠀');
    let result = COLLAPSE_SPACES.replace_all(&result, "⠀");
   
    return result.to_string();
}


static CMU_INDICATOR_REPLACEMENTS: phf::Map<&str, &str> = phf_map! {
    // "S" => "XXX",    // sans-serif -- from prefs
    "B" => "⠔",     // bold
    "𝔹" => "⠬",     // blackboard -- from prefs
    // "T" => "⠈",     // script
    "I" => "⠔",     // italic -- same as bold
    // "R" => "",      // roman
    // "E" => "⠰",     // English
    "1" => "⠐",     // Grade 1 symbol -- used here for a-j after number
    "L" => "",     // Letter left in to assist in locating letters
    "D" => "⠠",     // German (Gothic)
    "G" => "⠈",     // Greek
    "V" => "⠈⠬",    // Greek Variants
    // "H" => "⠠⠠",    // Hebrew
    // "U" => "⠈⠈",    // Russian
    "C" => "⠨",      // capital
    "𝐶" => "⠨",      // capital that never should get word indicator (from chemical element)
    "N" => "⠼",     // number indicator
    "𝑁" => "",      // continue number
    // "t" => "⠱",     // shape terminator
    "W" => "⠀",     // whitespace"
    "𝐖"=> "⠀",     // whitespace
    // "𝘄" => "⠀",    // add whitespace if char to the left has dots 1, 2, or 3 -- special rule handled separately, so commented out
    "s" => "",     // typeface single char indicator
    // "w" => "⠂",     // typeface word indicator
    // "e" => "⠄",     // typeface & capital terminator 
    // "o" => "",       // flag that what follows is an open indicator (used for standing alone rule)
    // "c" => "",       // flag that what follows is an close indicator (used for standing alone rule)
    // "b" => "",       // flag that what follows is an open or close indicator (used for standing alone rule)
    "," => "⠂",     // comma
    "." => "⠄",     // period
    "-" => "⠤",     // hyphen
    "—" => "⠤⠤",   // normal dash (2014) -- assume all normal dashes are unified here [RUEB appendix 3]
    // "―" => "⠐⠤⠤",  // long dash (2015) -- assume all long dashes are unified here [RUEB appendix 3]
    "#" => "⠼",      // signals to end/restart of numeric mode (mixed fractions)
};


fn cmu_cleanup(_pref_manager: Ref<PreferenceManager>, raw_braille: String) -> String {
    lazy_static! {
        static ref ADD_WHITE_SPACE: Regex = Regex::new(r"𝘄(.)|𝘄$").unwrap();
    }

    debug!("cmu_cleanup: start={}", raw_braille);
    // let result = typeface_to_word_mode(&raw_braille);

    // let result = result.replace("tW", "W");
    let result = raw_braille.replace("CG", "⠘")
                                .replace("𝔹C", "⠩")
                                .replace("DC", "⠰");
    // let result = result.replace("CC", "⠸"); 

    // these typeforms need to get pulled from user-prefs as they are transcriber-defined
    // let double_struck = pref_manager.pref_to_string("CMU_DoubleStruck");
    // let sans_serif = pref_manager.pref_to_string("CMU_SansSerif");
    // let fraktur = pref_manager.pref_to_string("CMU_Fraktur");

    debug!("Before remove mode changes: '{}'", &result);
    // This reuses the code just for getting rid of unnecessary "L"s and "N"s
    let result = remove_unneeded_mode_changes(&result, UEB_Mode::Grade1, UEB_Duration::Passage);
    let result = result.replace("𝑁N", "");
    debug!(" After remove mode changes: '{}'", &result);

    let result = REPLACE_INDICATORS.replace_all(&result, |cap: &Captures| {
        match CMU_INDICATOR_REPLACEMENTS.get(&cap[0]) {
            None => {error!("REPLACE_INDICATORS and CMU_INDICATOR_REPLACEMENTS are not in sync"); ""},
            Some(&ch) => ch,
        }
    });
    let result = ADD_WHITE_SPACE.replace_all(&result, |cap: &Captures| {
        if cap.get(1).is_none() {
            return "⠀".to_string();
        } else {
            // debug!("ADD_WHITE_SPACE match='{}', has left dots = {}", &cap[1], has_left_dots(cap[1].chars().next().unwrap()));
            let mut next_chars = cap[1].chars();
            let next_char = next_chars.next().unwrap();
            assert!(next_chars.next().is_none());
            return (if has_left_dots(next_char) {"⠀"} else {""}).to_string() + &cap[1];
        }
    });
    
    // Remove unicode blanks at start and end -- do this after the substitutions because ',' introduces spaces
    // let result = result.trim_start_matches('⠀').trim_end_matches('⠀');
    let result = COLLAPSE_SPACES.replace_all(&result, "⠀");
   
    return result.to_string();
    // return result.trim_end_matches('⠀').to_string();

    fn has_left_dots(ch: char) -> bool {
        // Unicode braille is set up so dot 1 is 2^0, dot 2 is 2^1, etc
        return ( (ch as u32 - 0x2800) >> 4 ) > 0;
    }
}

/************** Braille xpath functionality ***************/
use crate::canonicalize::{name, as_element, as_text};
use crate::xpath_functions::{is_leaf, IsBracketed, validate_one_node};
use sxd_document::dom::ParentOfChild;
use sxd_xpath::{Value, context, nodeset::*};
use sxd_xpath::function::{Function, Args};
use sxd_xpath::function::Error as XPathError;
use std::result::Result as StdResult;

pub struct NemethNestingChars;
const NEMETH_FRAC_LEVEL: &str = "data-nemeth-frac-level";    // name of attr where value is cached
const FIRST_CHILD_ONLY: &[&str] = &["mroot", "msub", "msup", "msubsup", "munder", "mover", "munderover", "mmultiscripts"];
impl NemethNestingChars {
    // returns a 'repeat_char' corresponding to the Nemeth rules for nesting
    // note: this value is likely one char too long because the starting fraction is counted
    fn nemeth_frac_value<'a>(node: &'a Element, repeat_char: &'a str) -> String {
        let children = node.children();
        let name = name(node);
        if is_leaf(*node) {
            return "".to_string();
        } else if name == "mfrac" {
            // have we already computed the value?
            if let Some(value) = node.attribute_value(NEMETH_FRAC_LEVEL) {
                return value.to_string();
            }

            let num_value = NemethNestingChars::nemeth_frac_value(&as_element(children[0]), repeat_char);
            let denom_value = NemethNestingChars::nemeth_frac_value(&as_element(children[1]), repeat_char);
            let mut max_value = if num_value.len() > denom_value.len() {num_value} else {denom_value};
            max_value += repeat_char;
            node.set_attribute_value(NEMETH_FRAC_LEVEL, &max_value);
            return max_value;
        } else if FIRST_CHILD_ONLY.contains(&name) {
            // only look at the base -- ignore scripts/index
            return NemethNestingChars::nemeth_frac_value(&as_element(children[0]), repeat_char);
        } else {
            let mut result = "".to_string();
            for child in children {
                let value = NemethNestingChars::nemeth_frac_value(&as_element(child), repeat_char);
                if value.len() > result.len() {
                    result = value;
                }
            }
            return result;
        }
    }

    fn nemeth_root_value<'a>(node: &'a Element, repeat_char: &'a str) -> StdResult<String, XPathError> {
        // returns the correct number of repeat_chars to use
        // note: because the highest count is toward the leaves and
        //    because this is a loop and not recursive, caching doesn't work without a lot of overhead
        let parent = node.parent().unwrap();
        if let ParentOfChild::Element(e) =  parent {
            let mut parent = e;
            let mut result = "".to_string();
            loop {
                let name = name(&parent);
                if name == "math" {
                    return Ok( result );
                }
                if name == "msqrt" || name == "mroot" {
                    result += repeat_char;
                }
                let parent_of_child = parent.parent().unwrap();
                if let ParentOfChild::Element(e) =  parent_of_child {
                    parent = e;
                } else {
                    return Err( sxd_xpath::function::Error::Other("Internal error in nemeth_root_value: didn't find 'math' tag".to_string()) );
                }
            }
        }
        return Err( XPathError::Other("Internal error in nemeth_root_value: didn't find 'math' tag".to_string()) );
    }
}

impl Function for NemethNestingChars {
/**
 * Returns a string with the correct number of nesting chars (could be an empty string)
 * @param(node) -- current node
 * @param(char) -- char (string) that should be repeated
 * Note: as a side effect, an attribute with the value so repeated calls to this or a child will be fast
 */
 fn evaluate<'d>(&self,
                        _context: &context::Evaluation<'_, 'd>,
                        args: Vec<Value<'d>>)
                        -> StdResult<Value<'d>, XPathError>
    {
        let mut args = Args(args);
        args.exactly(2)?;
        let repeat_char = args.pop_string()?;
        let node = crate::xpath_functions::validate_one_node(args.pop_nodeset()?, "NestingChars")?;
        if let Node::Element(el) = node {
            let name = name(&el);
            // it is likely a bug to call this one a non mfrac
            if name == "mfrac" {
                // because it is called on itself, the fraction is counted one too many times -- chop one off
                // this is slightly messy because we are chopping off a char, not a byte
                const BRAILLE_BYTE_LEN: usize = "⠹".len();      // all Unicode braille symbols have the same number of bytes
                return Ok( Value::String( NemethNestingChars::nemeth_frac_value(&el, &repeat_char)[BRAILLE_BYTE_LEN..].to_string() ) );
            } else if name == "msqrt" || name == "mroot" {
                return Ok( Value::String( NemethNestingChars::nemeth_root_value(&el, &repeat_char)? ) );
            } else {
                panic!("NestingChars chars should be used only on 'mfrac'. '{}' was passed in", name);
            }
        } else {
            // not an element, so nothing to do
            return Ok( Value::String("".to_string()) );
        }
    }
}

pub struct BrailleChars;
impl BrailleChars {
    // returns a string for the chars in the *leaf* node.
    // this string follows the Nemeth rules typefaces and deals with mathvariant
    //  which has partially turned chars to the alphanumeric block
    fn get_braille_chars(node: Element, code: &str, text_range: Option<Range<usize>>) -> StdResult<String, XPathError> {
        let result = match code {
            "Nemeth" => BrailleChars::get_braille_nemeth_chars(node, text_range),
            "UEB" => BrailleChars:: get_braille_ueb_chars(node, text_range),
            "CMU" => BrailleChars:: get_braille_cmu_chars(node, text_range),
            "Vietnam" => BrailleChars:: get_braille_vietnam_chars(node, text_range),
            _ => return Err(sxd_xpath::function::Error::Other(format!("get_braille_chars: unknown braille code '{}'", code)))
        };
        return match result {
            Ok(string) => Ok(string),
            Err(err) => return Err(sxd_xpath::function::Error::Other(err.to_string())),
        }
    }

    fn get_braille_nemeth_chars(node: Element, text_range: Option<Range<usize>>) -> Result<String> {
        lazy_static! {
            // To greatly simplify typeface/language generation, the chars have unique ASCII chars for them:
            // Typeface: S: sans-serif, B: bold, 𝔹: blackboard, T: script, I: italic, R: Roman
            // Language: E: English, D: German, G: Greek, V: Greek variants, H: Hebrew, U: Russian
            // Indicators: C: capital, L: letter, N: number, P: punctuation, M: multipurpose
            static ref PICK_APART_CHAR: Regex = 
                Regex::new(r"(?P<face>[SB𝔹TIR]*)(?P<lang>[EDGVHU]?)(?P<cap>C?)(?P<letter>L?)(?P<num>[N]?)(?P<char>.)").unwrap();
        }
        let math_variant = node.attribute_value("mathvariant");
        // FIX: cover all the options -- use phf::Map
        let  attr_typeface = match math_variant {
            None => "R",
            Some(variant) => match variant {
                "bold" => "B",
                "italic" => "I",
                "double-struck" => "𝔹",
                "script" => "T",
                "fraktur" => "D",
                "sans-serif" => "S",
                _ => "R",       // normal and unknown
            },
        };
        let text = BrailleChars::substring(as_text(node), &text_range);
        let braille_chars = crate::speech::braille_replace_chars(&text, node)?;
        // debug!("Nemeth chars: text='{}', braille_chars='{}'", &text, &braille_chars);
        
        // we want to pull the prefix (typeface, language) out to the front until a change happens
        // the same is true for number indicator
        // also true (sort of) for capitalization -- if all caps, use double cap in front (assume abbr or Roman Numeral)
        
        // we only care about this for numbers and identifiers/text, so we filter for only those
        let node_name = name(&node);
        let is_in_enclosed_list = node_name != "mo" && BrailleChars::is_in_enclosed_list(node);
        let is_mn_in_enclosed_list = is_in_enclosed_list && node_name == "mn";
        let mut typeface = "R".to_string();     // assumption is "R" and if attr or letter is different, something happens
        let mut is_all_caps = true;
        let mut is_all_caps_valid = false;      // all_caps only valid if we did a replacement
        let result = PICK_APART_CHAR.replace_all(&braille_chars, |caps: &Captures| {
            // debug!("  face: {:?}, lang: {:?}, num {:?}, letter: {:?}, cap: {:?}, char: {:?}",
            //        &caps["face"], &caps["lang"], &caps["num"], &caps["letter"], &caps["cap"], &caps["char"]);
            let mut nemeth_chars = "".to_string();
            let char_face = if caps["face"].is_empty() {attr_typeface} else {&caps["face"]};
            let typeface_changed =  typeface != char_face;
            if typeface_changed {
                typeface = char_face.to_string();   // needs to outlast this instance of the loop
                nemeth_chars += &typeface;
                nemeth_chars +=  &caps["lang"];
            } else {
                nemeth_chars +=  &caps["lang"];
            }
            // debug!("  typeface changed: {}, is_in_list: {}; num: {}", typeface_changed, is_in_enclosed_list, !caps["num"].is_empty());
            if !caps["num"].is_empty() && (typeface_changed || !is_mn_in_enclosed_list) {
                nemeth_chars += "N";
            }
            is_all_caps_valid = true;
            is_all_caps &= !&caps["cap"].is_empty();
            nemeth_chars += &caps["cap"];       // will be stripped later if all caps
            if is_in_enclosed_list {
                nemeth_chars += &caps["letter"].replace('L', "l");
            } else {
                nemeth_chars += &caps["letter"];
            }
            nemeth_chars += &caps["char"];
            return nemeth_chars;
        });
        // debug!("  result: {}", &result);
        let mut text_chars = text.chars();     // see if more than one char
        if is_all_caps_valid && is_all_caps && text_chars.next().is_some() &&  text_chars.next().is_some() {
            return Ok( "CC".to_string() + &result.replace('C', ""));
        } else {
            return Ok( result.to_string() );
        }
    }

    fn get_braille_ueb_chars(node: Element, text_range: Option<Range<usize>>) -> Result<String> {
        // Because in UEB typeforms and caps may extend for multiple tokens,
        //   this routine merely deals with the mathvariant attr.
        // Canonicalize has already transformed all chars it can to math alphanumerics, but not all have bold/italic 
        // The typeform/caps transforms to (potentially) word mode are handled later.
        lazy_static! {
            static ref HAS_TYPEFACE: Regex = Regex::new(".*?(double-struck|script|fraktur|sans-serif).*").unwrap();
            static ref PICK_APART_CHAR: Regex = 
                 Regex::new(r"(?P<bold>B??)(?P<italic>I??)(?P<face>[S𝔹TD]??)s??(?P<cap>C??)(?P<greek>G??)(?P<char>[NL].)").unwrap();
        }
    
        let math_variant = node.attribute_value("mathvariant");
        let text = BrailleChars::substring(as_text(node), &text_range);
        let braille_chars = crate::speech::braille_replace_chars(&text, node)?;

        // debug!("get_braille_ueb_chars: before/after unicode.yaml: '{}'/'{}'", text, braille_chars);
        if math_variant.is_none() {         // nothing we need to do
            return Ok(braille_chars);
        }
        // mathvariant could be "sans-serif-bold-italic" -- get the parts
        let math_variant = math_variant.unwrap();
        let bold = math_variant.contains("bold");
        let italic = math_variant.contains("italic");
        let typeface = match HAS_TYPEFACE.find(math_variant) {
            None => "",
            Some(m) => match m.as_str() {
                "double-struck" => "𝔹",
                "script" => "T",
                "fraktur" => "D",
                "sans-serif" => "S",
                //  don't consider monospace as a typeform
                _ => "",
            },
        };
        let result = PICK_APART_CHAR.replace_all(&braille_chars, |caps: &Captures| {
            // debug!("captures: {:?}", caps);
            // debug!("  bold: {:?}, italic: {:?}, face: {:?}, cap: {:?}, char: {:?}",
            //        &caps["bold"], &caps["italic"], &caps["face"], &caps["cap"], &caps["char"]);
            if bold || !caps["bold"].is_empty() {"B"} else {""}.to_string()
                + if italic || !caps["italic"].is_empty() {"I"} else {""}
                + if !&caps["face"].is_empty() {&caps["face"]} else {typeface}
                + &caps["cap"]
                + &caps["greek"]
                + &caps["char"]
        });
        return Ok(result.to_string())
    }

    fn get_braille_cmu_chars(node: Element, text_range: Option<Range<usize>>) -> Result<String> {
        // In CMU, we need to replace spaces used for number blocks with "."
        // For other numbers, we need to add "." to create digit blocks

        lazy_static! {
            // these all use ',' for decimal separators
            static ref NUMBER_WITH_SPACES: Regex = Regex::new(r"^[1-9]\d{0,2}( \d{3})*(,\d*)?$").unwrap();
            static ref NUMBER_WITH_BLOCKS: Regex = Regex::new(r"^([1-9]\d\d\d+)(,\d*)?$").unwrap();
    
            static ref HAS_TYPEFACE: Regex = Regex::new(".*?(double-struck|script|fraktur|sans-serif).*").unwrap();
            static ref PICK_APART_CHAR: Regex = 
                 Regex::new(r"(?P<bold>B??)(?P<italic>I??)(?P<face>[S𝔹TD]??)s??(?P<cap>C??)(?P<greek>G??)(?P<char>[NL].)").unwrap();
        }
    
        let math_variant = node.attribute_value("mathvariant");
        let text = BrailleChars::substring(as_text(node), &text_range);
        let text = add_separator(text);

        let braille_chars = crate::speech::braille_replace_chars(&text, node)?;

        // debug!("get_braille_ueb_chars: before/after unicode.yaml: '{}'/'{}'", text, braille_chars);
        if math_variant.is_none() {         // nothing we need to do
            return Ok(braille_chars);
        }
        // mathvariant could be "sans-serif-bold-italic" -- get the parts
        let math_variant = math_variant.unwrap();
        let bold = math_variant.contains("bold");
        let italic = math_variant.contains("italic");
        let typeface = match HAS_TYPEFACE.find(math_variant) {
            None => "",
            Some(m) => match m.as_str() {
                "double-struck" => "𝔹",
                "script" => "T",
                "fraktur" => "D",
                "sans-serif" => "S",
                //  don't consider monospace as a typeform
                _ => "",
            },
        };
        let result = PICK_APART_CHAR.replace_all(&braille_chars, |caps: &Captures| {
            // debug!("captures: {:?}", caps);
            // debug!("  bold: {:?}, italic: {:?}, face: {:?}, cap: {:?}, char: {:?}",
            //        &caps["bold"], &caps["italic"], &caps["face"], &caps["cap"], &caps["char"]);
            if bold || !caps["bold"].is_empty() {"B"} else {""}.to_string()
                + if italic || !caps["italic"].is_empty() {"I"} else {""}
                + if !&caps["face"].is_empty() {&caps["face"]} else {typeface}
                + &caps["cap"]
                + &caps["greek"]
                + &caps["char"]
        });
        return Ok(result.to_string());

        fn add_separator(text: String) -> String {
            use crate::definitions::DEFINITIONS;
            if NUMBER_WITH_SPACES.is_match(&text) {
                return text.replace(' ', ".");
            } else if let Some(text_without_arc) = text.strip_prefix("arc") {
                // "." after arc (7.5.3)
                let is_function_name = DEFINITIONS.with(|definitions| {
                    let definitions = definitions.borrow();
                    let set = definitions.get_hashset("CMUFunctionNames").unwrap();
                    return set.contains(&text);
                });
                if is_function_name {
                    return "arc.".to_string() + text_without_arc;
                }
            } 
            return text;  
        }
    }

    fn get_braille_vietnam_chars(node: Element, text_range: Option<Range<usize>>) -> Result<String> {
        // this is basically the same as for ueb except:
        // 1. we deal with switching '.' and ',' if in English style for numbers
        // 2. if it is identified as a Roman Numeral, we make all but the first char lower case because they shouldn't get a cap indicator
        if name(&node) == "mn" {
            // text of element is modified by these if needed
            lower_case_roman_numerals(node);
            switch_if_english_style_number(node);
        }
        return BrailleChars::get_braille_ueb_chars(node, text_range);

        fn lower_case_roman_numerals(mn_node: Element) {
            if mn_node.attribute("data-roman-numeral").is_some() {
                // if a roman numeral, all ASCII so we can optimize
                let text = as_text(mn_node);
                let mut new_text = String::from(&text[..1]);
                new_text.push_str(text[1..].to_ascii_lowercase().as_str());    // works for single char too
                mn_node.set_text(&new_text);
            }
        }
        fn switch_if_english_style_number(mn_node: Element) {
            let text = as_text(mn_node);
            let dot = text.find('.');
            let comma = text.find(',');
            match (dot, comma) {
                (None, None) => (),
                (Some(dot), Some(comma)) => {
                    if comma < dot {
                        // switch dot/comma -- using "\x01" as a temp when switching the the two chars
                        let switched = text.replace('.', "\x01").replace(',', ".").replace('\x01', ",");
                        mn_node.set_text(&switched);
                    }
                },
                (Some(dot), None) => {
                    // If it starts with a '.', a leading 0, or if there is only one '.' and not three chars after it
                    if dot==0 ||
                       (dot==1 && text.starts_with('0')) ||
                       (text[dot+1..].find('.').is_none() && text[dot+1..].len()!=3) {
                        mn_node.set_text(&text.replace('.', ","));
                    }
                },
                (None, Some(comma)) => {
                    // if there is more than one ",", than it can't be a decimal separator
                    if text[comma+1..].find(',').is_some() {
                        mn_node.set_text(&text.replace(',', "."));
                    }
                },
            }
        }

    }


    fn is_in_enclosed_list(node: Element) -> bool {
        // Nemeth Rule 10 defines an enclosed list:
        // 1: begins and ends with fence
        // 2: FIX: not implemented -- must contain no word, abbreviation, ordinal or plural ending
        // 3: function names or signs of shape and the signs which follow them are a single item (not a word)
        // 4: an item of the list may be an ellipsis or any sign used for omission
        // 5: no relational operator may appear within the list
        // 6: the list must have at least 2 items.
        //       Items are separated by commas, can not have other punctuation (except ellipsis and dash)
        let mut parent = node.parent().unwrap().element().unwrap(); // safe since 'math' is always at root
        while name(&parent) == "mrow" {
            if IsBracketed::is_bracketed(&parent, "", "", true, false) {
                for child in parent.children() {
                    if !child_meets_conditions(as_element(child)) {
                        return false;
                    }
                }
                return true;
            }
            parent = parent.parent().unwrap().element().unwrap();
        }
        return false;

        fn child_meets_conditions(node: Element) -> bool {
            let name = name(&node);
            return match name {
                "mi" | "mn" => true,
                "mo"  => !crate::canonicalize::is_relational_op(node),
                "mtext" => {
                    let text = as_text(node).trim();
                    return text=="?" || text=="-?-" || text.is_empty();   // various forms of "fill in missing content" (see also Nemeth_Rules.yaml, "omissions")
                },
                "mrow" => {
                    if IsBracketed::is_bracketed(&node, "", "", false, false) {
                        return child_meets_conditions(as_element(node.children()[1]));
                    } else {
                        for child in node.children() {
                            if !child_meets_conditions(as_element(child)) {
                                return false;
                            }
                        }
                    }  
                    true      
                },
                "menclose" => {
                    if let Some(notation) = node.attribute_value("notation") {
                        if notation != "bottom" || notation != "box" {
                            return false;
                        }
                        let child = as_element(node.children()[0]);     // menclose has exactly one child
                        return is_leaf(child) && as_text(child) == "?";
                    }
                    return false;
                },
                _ => {
                    for child in node.children() {
                        if !child_meets_conditions(as_element(child)) {
                            return false;
                        }
                    }
                    true
                },
            }
        }
    }

    /// Extract the `char`s from `str` within `range` (these are chars, not byte offsets)
    fn substring(str: &str, text_range: &Option<Range<usize>>) -> String {
        return match text_range {
            None => str.to_string(),
            Some(range) => str.chars().skip(range.start).take(range.end - range.start).collect(),
        }
    }
}

impl Function for BrailleChars {
    /**
     * Returns a string with the correct number of nesting chars (could be an empty string)
     * @param(node) -- current node or string
     * @param(char) -- char (string) that should be repeated
     * Note: as a side effect, an attribute with the value so repeated calls to this or a child will be fast
     */
     fn evaluate<'d>(&self,
                            context: &context::Evaluation<'_, 'd>,
                            args: Vec<Value<'d>>)
                            -> StdResult<Value<'d>, XPathError>
        {
            use crate::canonicalize::create_mathml_element;
            let mut args = Args(args);
            if let Err(e) = args.exactly(2).or_else(|_| args.exactly(4)) {
                return Err( XPathError::Other(format!("BrailleChars requires 2 or 4 args: {}", e)));
            };

            let range = if args.len() == 4 {
                let end = args.pop_number()? as usize - 1;      // non-inclusive at end, 0-based
                let start = args.pop_number()? as usize - 1;    // inclusive at start, a 0-based
                Some(start..end)
            } else {
                None
            };
            let braille_code = args.pop_string()?;
            let v: Value<'_> = args.0.pop().ok_or(XPathError::ArgumentMissing)?;
            let node = match v {
                Value::Nodeset(nodes) => {
                    validate_one_node(nodes, "BrailleChars")?.element().unwrap()
                },
                Value::Number(n) => {
                    let new_node = create_mathml_element(&context.node.document(), "mn");
                    new_node.set_text(&n.to_string());
                    new_node
                },
                Value::String(s) => {
                    let new_node = create_mathml_element(&context.node.document(), "mi");   // FIX: try to guess mi vs mo???
                    new_node.set_text(&s);
                    new_node
                },
                _ => {
                    return Ok( Value::String("".to_string()) ) // not an element, so nothing to do
                },
            };
    
            if !is_leaf(node) {
                return Err( XPathError::Other(format!("BrailleChars called on non-leaf element '{}'", mml_to_string(&node))) );
            }
            return Ok( Value::String( BrailleChars::get_braille_chars(node, &braille_code, range)? ) );
        }
    }
    
    
#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use crate::init_logger;
    use crate::interface::*;
    
    #[test]
    fn ueb_highlight_24() -> Result<()> {       // issue 24
        let mathml_str = "<math display='block' id='id-0'>
            <mrow id='id-1'>
                <mn id='id-2'>4</mn>
                <mo id='id-3'>&#x2062;</mo>
                <mi id='id-4'>a</mi>
                <mo id='id-5'>&#x2062;</mo>
                <mi id='id-6'>c</mi>
            </mrow>
        </math>";
        crate::interface::set_rules_dir(super::super::abs_rules_dir_path()).unwrap();
        set_mathml(mathml_str.to_string()).unwrap();
        set_preference("BrailleCode".to_string(), "UEB".to_string()).unwrap();
        set_preference("BrailleNavHighlight".to_string(), "All".to_string()).unwrap();
        let braille = get_braille("id-2".to_string())?;
        assert_eq!("⣼⣙⠰⠁⠉", braille);
        let braille = get_braille("id-4".to_string())?;
        assert_eq!("⠼⠙⣰⣁⠉", braille);
        return Ok( () );
    }
    
    #[test]
    #[allow(non_snake_case)]
    fn test_UEB_start_mode() -> Result<()> {
        let mathml_str = "<math><msup><mi>x</mi><mi>n</mi></msup></math>";
        crate::interface::set_rules_dir(super::super::abs_rules_dir_path()).unwrap();
        set_mathml(mathml_str.to_string()).unwrap();
        set_preference("BrailleCode".to_string(), "UEB".to_string()).unwrap();
        set_preference("UEB_START_MODE".to_string(), "Grade2".to_string()).unwrap();
        let braille = get_braille("".to_string())?;
        assert_eq!("⠭⠰⠔⠝", braille, "Grade2");
        set_preference("UEB_START_MODE".to_string(), "Grade1".to_string()).unwrap();
        let braille = get_braille("".to_string())?;
        assert_eq!("⠭⠔⠝", braille, "Grade1");
        return Ok( () );
    }
}