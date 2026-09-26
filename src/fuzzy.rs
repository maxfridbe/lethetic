//! fzf-style fuzzy matching for pickers: every query character must appear
//! in order; consecutive runs, word starts and early matches score higher.

/// Score `text` against one query term, or `None` when it does not match.
fn score_term(term: &[char], text: &[char]) -> Option<i64> {
    let first = *term.first()?;
    let mut best: Option<i64> = None;
    // Try every start position of the first character and keep the best
    // greedy alignment from there.
    for start in (0..text.len()).filter(|&index| text[index] == first) {
        let mut score = 0_i64;
        let mut position = start;
        let mut previous: Option<usize> = None;
        let mut matched = true;
        for &needle in term {
            let Some(offset) = text[position..].iter().position(|&c| c == needle) else {
                matched = false;
                break;
            };
            let index = position + offset;
            score += 16;
            let word_start = index == 0 || !text[index - 1].is_alphanumeric();
            if word_start {
                score += 12;
            }
            match previous {
                Some(last) if index == last + 1 => score += 10,
                Some(last) => score -= (index - last - 1).min(12) as i64,
                None => score -= (index as i64).min(10),
            }
            previous = Some(index);
            position = index + 1;
        }
        if matched {
            best = Some(best.map_or(score, |current| current.max(score)));
        }
    }
    best
}

/// Match a whitespace-separated query against `text`; every term must
/// match. An empty query matches everything with score 0.
pub fn score(query: &str, text: &str) -> Option<i64> {
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut total = 0;
    for term in query.split_whitespace() {
        let term: Vec<char> = term.to_lowercase().chars().collect();
        total += score_term(&term, &text)?;
    }
    Some(total)
}

/// Rank `items` by the best of their primary text and (at half weight)
/// secondary text. Non-matches are dropped; ties keep the original order.
pub fn rank<T: Clone>(
    query: &str,
    items: &[T],
    primary: impl Fn(&T) -> String,
    secondary: impl Fn(&T) -> String,
) -> Vec<T> {
    if query.trim().is_empty() {
        return items.to_vec();
    }
    let mut scored: Vec<(i64, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let main = score(query, &primary(item));
            let extra = score(query, &secondary(item)).map(|value| value / 2);
            main.max(extra).map(|value| (value, index))
        })
        .collect();
    scored.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    scored
        .into_iter()
        .map(|(_, index)| items[index].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_matches_rank_word_starts_and_runs_first() {
        assert!(score("tdl", "Toggle Todo List").is_some());
        assert!(score("xyz", "Toggle Todo List").is_none());
        let labels = [
            "Hotkeys",
            "Models",
            "Remote Control: start",
            "Delete Python runtime/packages",
        ];
        let ranked = rank("mod", &labels, |s| s.to_string(), |_| String::new());
        assert_eq!(ranked[0], "Models");
        let ranked = rank("rc", &labels, |s| s.to_string(), |_| String::new());
        assert_eq!(ranked[0], "Remote Control: start");
        assert_eq!(
            rank("", &labels, |s| s.to_string(), |_| String::new()).len(),
            4
        );
        let ranked = rank("py pack", &labels, |s| s.to_string(), |_| String::new());
        assert_eq!(ranked, ["Delete Python runtime/packages"]);
    }
}
