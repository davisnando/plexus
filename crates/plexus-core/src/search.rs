//! Tiny in-process BM25 used by meta-mode `search_tools`.

use std::collections::HashMap;

const K1: f64 = 1.2;
const B: f64 = 0.75;

pub struct Bm25<T> {
    docs: Vec<(T, HashMap<String, usize>, usize)>,
    doc_freq: HashMap<String, usize>,
    avg_len: f64,
}

pub fn tokenize(text: &str) -> Vec<String> {
    // Split camelCase too, so `createIssue` matches "create issue".
    let mut spaced = String::with_capacity(text.len() + 8);
    let mut prev_lower = false;
    for c in text.chars() {
        if c.is_uppercase() && prev_lower {
            spaced.push(' ');
        }
        prev_lower = c.is_lowercase();
        spaced.push(c);
    }
    spaced
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| {
            // Naive plural folding: "issues" matches "issue".
            let mut t = t.to_lowercase();
            if t.len() > 3 && t.ends_with('s') && !t.ends_with("ss") {
                t.pop();
            }
            t
        })
        .collect()
}

impl<T> Bm25<T> {
    pub fn new(docs: impl IntoIterator<Item = (T, String)>) -> Self {
        let mut doc_freq: HashMap<String, usize> = HashMap::new();
        let docs: Vec<_> = docs
            .into_iter()
            .map(|(item, text)| {
                let tokens = tokenize(&text);
                let mut tf: HashMap<String, usize> = HashMap::new();
                for t in &tokens {
                    *tf.entry(t.clone()).or_default() += 1;
                }
                for t in tf.keys() {
                    *doc_freq.entry(t.clone()).or_default() += 1;
                }
                (item, tf, tokens.len())
            })
            .collect();
        let avg_len = if docs.is_empty() {
            0.0
        } else {
            docs.iter().map(|d| d.2).sum::<usize>() as f64 / docs.len() as f64
        };
        Self {
            docs,
            doc_freq,
            avg_len,
        }
    }

    /// Items ranked by score, best first; zero-score items are dropped.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(&T, f64)> {
        let terms = tokenize(query);
        let n = self.docs.len() as f64;
        let mut scored: Vec<(&T, f64)> = self
            .docs
            .iter()
            .map(|(item, tf, len)| {
                let score = terms
                    .iter()
                    .map(|t| {
                        let Some(&f) = tf.get(t) else { return 0.0 };
                        let df = self.doc_freq.get(t).copied().unwrap_or(0) as f64;
                        let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                        let f = f as f64;
                        idf * f * (K1 + 1.0)
                            / (f + K1 * (1.0 - B + B * *len as f64 / self.avg_len.max(1.0)))
                    })
                    .sum::<f64>();
                (item, score)
            })
            .filter(|(_, s)| *s > 0.0)
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(limit);
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_relevant_first() {
        let index = Bm25::new([
            ("jira__createIssue", "jira createIssue Create a new Jira issue".to_string()),
            ("sentry__list_issues", "sentry list_issues List Sentry issues for a project".to_string()),
            ("github__create_pr", "github create_pr Open a pull request".to_string()),
        ]);
        let hits = index.search("create issue", 10);
        assert_eq!(*hits[0].0, "jira__createIssue");
        assert_eq!(hits.len(), 3);
        assert!(index.search("zzz", 10).is_empty());
    }
}
