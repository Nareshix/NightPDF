#[derive(Clone)]
pub struct Hit {
    pub page: usize,
    pub start: usize,
    pub end: usize,
}

pub fn find_all(pages: &[String], query: &str) -> Vec<Hit> {
    if query.is_empty() { return vec![]; }
    let q = query.to_lowercase();
    let mut hits = Vec::new();
    for (page, text) in pages.iter().enumerate() {
        let lower = text.to_lowercase();
        let mut offset = 0;
        while let Some(pos) = lower[offset..].find(&q) {
            let start = offset + pos;
            hits.push(Hit { page, start, end: start + query.len() });
            offset = start + 1;
        }
    }
    hits
}