//! Découpage en phrases d'un texte qui arrive par fragments, pour prononcer chaque
//! phrase dès qu'elle est complète.
//!
//! Une phrase finit sur `.`, `!`, `?` ou `…` suivis d'un blanc, ou sur un saut de ligne.
//! Tant que ce qui suit la ponctuation n'est pas encore arrivé, on attend : « 3. » peut
//! devenir « 3.5 ». Les abréviations courantes du français ne coupent pas.

/// Titres toujours suivis d'un nom : jamais une fin de phrase (casse exacte).
const TITLES: &[&str] = &[
    "M", "MM", "Mme", "Mmes", "Mlle", "Mlles", "Dr", "Pr", "Me", "Mgr", "St", "Ste",
];
/// Abréviations presque jamais en fin de phrase (casse ignorée).
const NON_FINAL: &[&str] = &[
    "cf", "ex", "env", "vol", "fig", "art", "chap", "av", "apr", "p", "pp", "no", "n°", "tél",
    "réf", "vs", "approx",
];
/// Au-delà, on coupe quand même (sur une virgule si possible) pour ne pas faire attendre.
const MAX_PENDING_CHARS: usize = 300;

const TERMINATORS: &[char] = &['.', '!', '?', '…'];
const CLOSERS: &[char] = &['»', '"', '”', '’', '\'', ')', ']'];

#[derive(Debug)]
pub struct SentenceSplitter {
    buffer: String,
    /// Le tampon commence-t-il une ligne ? Sert à reconnaître « 1. » en tête de liste.
    line_start: bool,
}

impl Default for SentenceSplitter {
    fn default() -> Self {
        Self {
            buffer: String::new(),
            line_start: true,
        }
    }
}

impl SentenceSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute un fragment et rend les phrases désormais complètes.
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.buffer.push_str(delta);
        let mut sentences = Vec::new();
        while let Some(end) =
            find_boundary(&self.buffer, self.line_start).or_else(|| overflow_cut(&self.buffer))
        {
            let piece: String = self.buffer.drain(..end).collect();
            self.line_start = piece.ends_with('\n');
            if let Some(sentence) = speakable(&piece) {
                sentences.push(sentence);
            }
        }
        sentences
    }

    /// Fin du flux : rend ce qui reste, même sans ponctuation finale.
    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buffer);
        self.line_start = true;
        speakable(&rest)
    }

    /// Oublie le texte en attente (barge-in).
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.line_start = true;
    }
}

/// Ce que l'abréviation devant un point permet.
enum AfterDot {
    Split,
    NeverSplit,
    /// « etc. », « J.-C. » : fin de phrase seulement si une majuscule suit.
    SplitBeforeUppercase,
}

/// Position (en octets) de la fin de la première phrase complète de `text`.
fn find_boundary(text: &str, line_start: bool) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let (position, c) = chars[i];
        if c == '\n' {
            return Some(position + 1);
        }
        if !TERMINATORS.contains(&c) {
            i += 1;
            continue;
        }
        let mut end = i;
        while end < n && TERMINATORS.contains(&chars[end].1) {
            end += 1;
        }
        let single_dot = end == i + 1 && c == '.';
        let quote_open = chars[..i].iter().filter(|(_, c)| *c == '«').count()
            > chars[..i].iter().filter(|(_, c)| *c == '»').count();
        end = skip_closers(&chars, end, quote_open)?;
        if end == n {
            return None; // la suite décidera
        }
        if !chars[end].1.is_whitespace() {
            // « 3.5 », « argos.dev », « ?! » déjà consommé : pas une fin de phrase.
            i = end;
            continue;
        }
        let rule = if single_dot {
            word_rule(&chars, i, line_start)
        } else {
            AfterDot::Split
        };
        match rule {
            AfterDot::Split => return Some(chars[end].0),
            AfterDot::NeverSplit => {}
            AfterDot::SplitBeforeUppercase => {
                let next = chars[end..].iter().find(|(_, c)| !c.is_whitespace());
                match next {
                    None => return None,
                    Some((_, next)) if next.is_uppercase() => return Some(chars[end].0),
                    Some(_) => {}
                }
            }
        }
        i = end;
    }
    None
}

/// Passe les guillemets et parenthèses fermants après la ponctuation, y compris le
/// « » » français précédé d'une espace quand un guillemet est ouvert. `None` : on ne
/// sait pas encore.
fn skip_closers(chars: &[(usize, char)], mut end: usize, quote_open: bool) -> Option<usize> {
    let n = chars.len();
    while end < n && CLOSERS.contains(&chars[end].1) {
        end += 1;
    }
    if !quote_open {
        return Some(end);
    }
    let mut ahead = end;
    while ahead < n && matches!(chars[ahead].1, ' ' | '\u{a0}' | '\u{202f}') {
        ahead += 1;
    }
    if ahead == n && ahead > end {
        return None; // un « » » peut encore arriver
    }
    if ahead < n && chars[ahead].1 == '»' {
        end = ahead + 1;
        while end < n && CLOSERS.contains(&chars[end].1) {
            end += 1;
        }
    }
    Some(end)
}

/// Règle du mot qui précède le point en `dot`.
fn word_rule(chars: &[(usize, char)], dot: usize, line_start: bool) -> AfterDot {
    let mut start = dot;
    while start > 0 && !chars[start - 1].1.is_whitespace() {
        start -= 1;
    }
    let raw: String = chars[start..dot].iter().map(|(_, c)| c).collect();
    let word = raw.trim_start_matches(['(', '«', '"', '“', '\'']);
    if word.is_empty() {
        return AfterDot::Split;
    }
    let mut letters = word.chars();
    let single_initial =
        matches!((letters.next(), letters.next()), (Some(c), None) if c.is_uppercase());
    if TITLES.contains(&word) || single_initial {
        return AfterDot::NeverSplit;
    }
    let lower = word.to_lowercase();
    if NON_FINAL.contains(&lower.as_str()) {
        return AfterDot::NeverSplit;
    }
    if lower == "etc" || word.contains('.') {
        return AfterDot::SplitBeforeUppercase;
    }
    // « 1. » en tête de ligne : un numéro de liste, pas une phrase.
    if word.chars().all(|c| c.is_ascii_digit()) {
        let before = &chars[..start];
        let blank_until_line = before
            .iter()
            .rev()
            .take_while(|(_, c)| *c != '\n')
            .all(|(_, c)| c.is_whitespace());
        let at_line_start = before.iter().any(|(_, c)| *c == '\n') || line_start;
        if blank_until_line && at_line_start {
            return AfterDot::NeverSplit;
        }
    }
    AfterDot::Split
}

/// Coupe un tampon trop long : après la dernière virgule (ou `;`, `:`) suivie d'un
/// blanc, sinon au dernier blanc.
fn overflow_cut(text: &str) -> Option<usize> {
    if text.chars().count() <= MAX_PENDING_CHARS {
        return None;
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let pause = chars
        .windows(2)
        .rposition(|pair| matches!(pair[0].1, ',' | ';' | ':') && pair[1].1.is_whitespace());
    if let Some(index) = pause {
        return Some(chars[index + 1].0);
    }
    chars
        .iter()
        .rposition(|(_, c)| c.is_whitespace())
        .filter(|&index| index > 0)
        .map(|index| chars[index].0)
}

/// Texte prêt pour la synthèse : sans balisage Markdown, ou `None` s'il n'y a rien à dire.
fn speakable(piece: &str) -> Option<String> {
    let text = piece.replace(['*', '`'], "").replace("__", "");
    // Titre (`## `) puis puce de liste éventuelle en tête.
    let mut rest = text.trim_start().trim_start_matches('#').trim_start();
    for marker in ["- ", "• "] {
        if let Some(stripped) = rest.strip_prefix(marker) {
            rest = stripped;
            break;
        }
    }
    let sentence = rest.split_whitespace().collect::<Vec<_>>().join(" ");
    sentence
        .chars()
        .any(char::is_alphanumeric)
        .then_some(sentence)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Découpe `text` livré en fragments de `size` caractères, comme un flux de LLM.
    fn split(text: &str, size: usize) -> Vec<String> {
        let mut splitter = SentenceSplitter::new();
        let chars: Vec<char> = text.chars().collect();
        let mut sentences = Vec::new();
        for piece in chars.chunks(size) {
            sentences.extend(splitter.push(&piece.iter().collect::<String>()));
        }
        sentences.extend(splitter.finish());
        sentences
    }

    /// Le découpage ne dépend pas de la taille des fragments.
    fn check(text: &str, expected: &[&str]) {
        for size in [1, 2, 3, 7, 1_000] {
            assert_eq!(split(text, size), expected, "fragments de {size}");
        }
    }

    #[test]
    fn simple_sentences() {
        check(
            "Bonjour. Comment vas-tu ? Très bien !",
            &["Bonjour.", "Comment vas-tu ?", "Très bien !"],
        );
    }

    #[test]
    fn a_sentence_is_released_as_soon_as_it_is_complete() {
        let mut splitter = SentenceSplitter::new();
        assert!(splitter.push("Je regarde").is_empty());
        // Le point seul ne suffit pas : « 3. » pourrait devenir « 3.5 ».
        assert!(splitter.push(" ça.").is_empty());
        assert_eq!(splitter.push(" Ensuite"), vec!["Je regarde ça."]);
        assert_eq!(splitter.finish(), Some("Ensuite".into()));
        assert_eq!(splitter.finish(), None);
    }

    #[test]
    fn titles_and_initials_do_not_end_a_sentence() {
        check(
            "M. Dupont et Mme Durand sont là. Le Dr. House aussi. J. K. Rowling écrit.",
            &[
                "M. Dupont et Mme Durand sont là.",
                "Le Dr. House aussi.",
                "J. K. Rowling écrit.",
            ],
        );
    }

    #[test]
    fn etc_ends_a_sentence_only_before_an_uppercase() {
        check(
            "Des pommes, des poires, etc. Ensuite on rentre.",
            &["Des pommes, des poires, etc.", "Ensuite on rentre."],
        );
        check(
            "Des pommes, etc. et des poires.",
            &["Des pommes, etc. et des poires."],
        );
        check(
            "En 52 av. J.-C. César arrive. Voir p. 12.",
            &["En 52 av. J.-C.", "César arrive.", "Voir p. 12."],
        );
    }

    #[test]
    fn decimals_and_domains_are_not_cut() {
        check(
            "Il fait 3.5 degrés, soit 3,5. Voir argos.dev pour la suite.",
            &[
                "Il fait 3.5 degrés, soit 3,5.",
                "Voir argos.dev pour la suite.",
            ],
        );
    }

    #[test]
    fn punctuation_runs_ellipses_and_quotes() {
        check(
            "Vraiment ?! Oui… Bon... Il a dit « Bonjour. » Puis (il est parti.) Fin",
            &[
                "Vraiment ?!",
                "Oui…",
                "Bon...",
                "Il a dit « Bonjour. »",
                "Puis (il est parti.)",
                "Fin",
            ],
        );
    }

    #[test]
    fn lines_and_numbered_lists() {
        check(
            "Voici le plan :\n1. Lire le mail.\n2. Répondre\n\nC'est tout. En 2024. Puis",
            &[
                "Voici le plan :",
                "1. Lire le mail.",
                "2. Répondre",
                "C'est tout.",
                "En 2024.",
                "Puis",
            ],
        );
    }

    #[test]
    fn markdown_is_not_read_aloud() {
        check(
            "## Titre\n**Important** : lis `ceci`.\n- un point\n---\n",
            &["Titre", "Important : lis ceci.", "un point"],
        );
    }

    #[test]
    fn a_long_run_without_punctuation_is_cut_on_a_pause() {
        let clause = "une proposition assez longue qui continue encore";
        let text = format!(
            "{clause}, {clause}, {clause}, {clause}, {clause}, {clause}, {clause} et la fin"
        );
        let sentences = split(&text, 5);
        assert!(sentences.len() >= 2, "{sentences:?}");
        assert!(sentences[0].ends_with(','), "{sentences:?}");
        assert!(
            sentences
                .iter()
                .all(|s| s.chars().count() <= MAX_PENDING_CHARS)
        );
        assert_eq!(sentences.join(" "), text);
    }

    #[test]
    fn clear_forgets_the_pending_text() {
        let mut splitter = SentenceSplitter::new();
        assert!(splitter.push("Une phrase coupée").is_empty());
        splitter.clear();
        assert_eq!(splitter.push("Autre chose. "), vec!["Autre chose."]);
        assert_eq!(splitter.finish(), None);
    }
}
