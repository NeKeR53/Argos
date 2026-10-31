//! Mot de déclenchement de l'écoute mains libres, reconnu dans la transcription de
//! Whisper : « Argos, quelle heure est-il ? » est une demande, une phrase sans le mot
//! n'en est pas une. Whisper écrit le nom à sa façon (« Argo », « Hargos »…) : la
//! comparaison tolère une lettre d'écart et quelques mots d'appel devant (« Dis Argos »).
//!
//! Le filtre d'écho écarte aussi ce que la synthèse vient de dire, entendu par le micro.

/// Mots d'appel qui peuvent précéder le mot de déclenchement.
const CALLS: [&str; 7] = ["ok", "okay", "dis", "he", "eh", "hey", "allo"];

/// Si `transcript` commence par le mot de déclenchement, rend la demande qui suit
/// (vide si le mot est seul). `None` sinon.
pub fn strip_wake_word(transcript: &str, wake: &str) -> Option<String> {
    let wake = normalize(wake);
    if wake.is_empty() {
        return None;
    }
    let wake_words: Vec<&str> = wake.split(' ').collect();
    let words: Vec<&str> = transcript.split_whitespace().collect();
    let mut start = 0;
    while start < words.len() && start < 2 && CALLS.contains(&normalize(words[start]).as_str()) {
        start += 1;
    }
    // Le nom en un mot, ou coupé en deux par Whisper (« Ar gos »).
    for span in [wake_words.len(), wake_words.len() + 1] {
        let end = start + span;
        if end > words.len() {
            continue;
        }
        let heard = normalize(&words[start..end].join(" "));
        if close(&heard.replace(' ', ""), &wake.replace(' ', "")) {
            let rest = words[end..].join(" ");
            let rest = rest.trim_start_matches(|c: char| c.is_ascii_punctuation() || c == '…');
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Vrai si `transcript` ressemble à ce que la synthèse a dit récemment : le haut-parleur
/// entendu par le micro, pas l'utilisateur.
pub fn is_echo(transcript: &str, recently_spoken: &[String]) -> bool {
    let heard: Vec<String> = normalize(transcript)
        .split(' ')
        .filter(|word| word.len() > 2)
        .map(String::from)
        .collect();
    if heard.is_empty() {
        return false;
    }
    let spoken = normalize(&recently_spoken.join(" "));
    let spoken: Vec<&str> = spoken.split(' ').collect();
    let shared = heard
        .iter()
        .filter(|word| spoken.contains(&word.as_str()))
        .count();
    shared * 10 >= heard.len() * 6
}

/// Minuscules sans accents ni ponctuation, mots séparés par une espace.
fn normalize(text: &str) -> String {
    let plain: String = text
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'À' | 'Â' | 'Ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
            'î' | 'ï' | 'Î' | 'Ï' => 'i',
            'ô' | 'ö' | 'Ô' | 'Ö' => 'o',
            'ù' | 'û' | 'ü' | 'Ù' | 'Û' | 'Ü' => 'u',
            'ç' | 'Ç' => 'c',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    plain
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Une lettre d'écart au plus (deux pour un nom long), plus un « h » muet devant ou un
/// « s » final que Whisper ajoute ou retire.
fn close(heard: &str, wake: &str) -> bool {
    if heard.is_empty() {
        return false;
    }
    let heard = heard
        .strip_prefix('h')
        .filter(|_| !wake.starts_with('h'))
        .unwrap_or(heard);
    let budget = if wake.chars().count() >= 8 { 2 } else { 1 };
    let trimmed = |s: &str| s.trim_end_matches(['s', 'e']).to_string();
    distance(heard, wake) <= budget || trimmed(heard) == trimmed(wake)
}

/// Distance de Levenshtein, sur les caractères.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current.push(
                (previous[j] + cost)
                    .min(previous[j + 1] + 1)
                    .min(current[j] + 1),
            );
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wake_word_opens_a_request_whatever_whisper_wrote() {
        for (heard, request) in [
            ("Argos, quelle heure est-il ?", "quelle heure est-il ?"),
            ("argos quelle heure est-il", "quelle heure est-il"),
            ("Argo, allume le salon.", "allume le salon."),
            ("Hargos ? Allume le salon", "Allume le salon"),
            ("Argus allume le salon", "allume le salon"),
            ("Dis Argos, allume le salon", "allume le salon"),
            ("Ok, Argos… allume le salon", "allume le salon"),
            ("Ar gos, allume le salon", "allume le salon"),
            ("Argos.", ""),
            ("Argos", ""),
        ] {
            assert_eq!(
                strip_wake_word(heard, "Argos").as_deref(),
                Some(request),
                "{heard}"
            );
        }
    }

    #[test]
    fn a_sentence_without_the_wake_word_is_not_a_request() {
        for heard in [
            "Quelle heure est-il ?",
            "Les arbres du jardin",
            "Je suis Argos, votre assistant.",
            "Ok, allume le salon",
            "",
        ] {
            assert_eq!(strip_wake_word(heard, "Argos"), None, "{heard}");
        }
        assert_eq!(strip_wake_word("Argos, salut", ""), None);
    }

    #[test]
    fn another_wake_word_can_be_chosen() {
        assert_eq!(
            strip_wake_word("Hé Jarvis, lance la musique", "Jarvis").as_deref(),
            Some("lance la musique")
        );
        assert_eq!(
            strip_wake_word("Mon assistant, la météo", "Mon assistant").as_deref(),
            Some("la météo")
        );
    }

    #[test]
    fn what_the_speaker_just_said_is_an_echo() {
        let spoken = vec![
            "Il fait dix-huit degrés dans le salon.".to_string(),
            "La fenêtre de la cuisine est ouverte.".to_string(),
        ];
        assert!(is_echo("il fait 18 degrés dans le salon", &spoken));
        assert!(is_echo(
            "Argos ? La fenêtre de la cuisine est ouverte",
            &spoken
        ));
        assert!(!is_echo("Argos, ferme la fenêtre", &spoken));
        assert!(!is_echo("Argos, quelle heure est-il ?", &spoken));
        assert!(!is_echo("", &spoken));
        assert!(!is_echo("salon", &[]));
    }
}
