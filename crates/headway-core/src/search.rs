//! Finding a station by typing at it.

use crate::network::{Network, StationId};

/// Folds a name for matching: lowercased, with common Latin diacritics reduced
/// to their base letters.
///
/// This map is full of names like `Žižkov`, `Ružinov Road` and `Ombelé`, and
/// nobody reaching for a station is going to type the diacritics.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' | 'ă' | 'ą' => out.push('a'),
            'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => out.push('c'),
            'ď' | 'đ' => out.push('d'),
            'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => out.push('e'),
            'ĝ' | 'ğ' | 'ġ' | 'ģ' => out.push('g'),
            'ĥ' | 'ħ' => out.push('h'),
            'í' | 'ì' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => out.push('i'),
            'ĵ' => out.push('j'),
            'ķ' => out.push('k'),
            'ĺ' | 'ļ' | 'ľ' | 'ł' => out.push('l'),
            'ñ' | 'ń' | 'ņ' | 'ň' => out.push('n'),
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' | 'ō' | 'ŏ' | 'ő' => out.push('o'),
            'ŕ' | 'ŗ' | 'ř' => out.push('r'),
            'ś' | 'ŝ' | 'ş' | 'š' => out.push('s'),
            'ţ' | 'ť' | 'ŧ' => out.push('t'),
            'ú' | 'ù' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => out.push('u'),
            'ŵ' => out.push('w'),
            'ý' | 'ÿ' | 'ŷ' => out.push('y'),
            'ź' | 'ż' | 'ž' => out.push('z'),
            'ß' => out.push_str("ss"),
            'æ' => out.push_str("ae"),
            'œ' => out.push_str("oe"),
            other => out.push(other),
        }
    }
    out
}

/// How well a name answers a query. Lower sorts first.
fn rank(name_folded: &str, needle: &str) -> Option<u8> {
    if name_folded == needle {
        Some(0)
    } else if name_folded.starts_with(needle) {
        Some(1)
    } else if name_folded.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(needle)) {
        // "yard" should find "Navy Yard"
        Some(2)
    } else if name_folded.contains(needle) {
        Some(3)
    } else {
        None
    }
}

impl Network {
    /// Named stations matching `query`, best first, at most `limit` of them.
    ///
    /// Unnamed stations are unreachable by search by definition; they stay in
    /// the graph but cannot be typed at.
    pub fn search(&self, query: &str, limit: usize) -> Vec<StationId> {
        let needle = fold(query.trim());
        if needle.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut hits: Vec<(u8, usize, &str, StationId)> = self
            .stations
            .iter()
            .filter_map(|s| {
                let name = s.name.as_deref()?;
                let score = rank(&fold(name), &needle)?;
                Some((score, name.chars().count(), name, s.id))
            })
            .collect();
        // shorter names first within a rank, then alphabetically, so the order
        // never depends on how the stations happened to be laid out
        hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(b.2)).then(a.3.cmp(&b.3)));
        hits.into_iter().take(limit).map(|hit| hit.3).collect()
    }

    /// The station with exactly this name, if exactly one has it.
    pub fn station_named(&self, name: &str) -> Option<StationId> {
        let wanted = fold(name.trim());
        let mut found = self
            .stations
            .iter()
            .filter(|s| s.name.as_deref().is_some_and(|n| fold(n) == wanted));
        let first = found.next()?;
        found.next().is_none().then_some(first.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    const MAP: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0,0,24,12">
      <g>
        <text>Key to lines</text>
        <path d="M0.5 0.5l1 0" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <text>Only Line</text>
      </g>
      <g>
        <path d="M1.5 1.5L2.5 1.5L3.5 1.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M3.5 1.5L4.5 1.5L5.5 1.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M5.5 1.5L6.5 1.5L7.5 1.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M7.5 1.5L8.5 1.5L9.5 1.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
      </g>
      <g>
        <path d="M1.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Žižkov</text></g>
        <path d="M3.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Navy</text><text>Yard</text></g>
        <path d="M5.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Navy Yard East</text></g>
        <path d="M7.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Ružinov Road</text></g>
        <path d="M9.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g></g>
      </g>
    </svg>"##;

    fn names(net: &Network, query: &str) -> Vec<String> {
        net.search(query, 8).iter().map(|id| net.label(*id)).collect()
    }

    #[test]
    fn folds_diacritics_so_a_plain_keyboard_reaches_them() {
        assert_eq!(fold("Žižkov"), "zizkov");
        assert_eq!(fold("Ružinov Road"), "ruzinov road");
        assert_eq!(fold("Ombelé"), "ombele");
        assert_eq!(fold("Straße"), "strasse");
        let net = parse(MAP).unwrap();
        assert_eq!(names(&net, "zizkov"), ["Žižkov"]);
        assert_eq!(names(&net, "RUZINOV"), ["Ružinov Road"]);
    }

    #[test]
    fn an_exact_name_outranks_a_longer_one_containing_it() {
        let net = parse(MAP).unwrap();
        assert_eq!(names(&net, "Navy Yard"), ["Navy Yard", "Navy Yard East"]);
    }

    #[test]
    fn matches_on_any_word_not_just_the_first() {
        let net = parse(MAP).unwrap();
        assert_eq!(names(&net, "yard"), ["Navy Yard", "Navy Yard East"]);
        assert_eq!(names(&net, "road"), ["Ružinov Road"]);
    }

    #[test]
    fn prefixes_beat_mid_word_matches() {
        let net = parse(MAP).unwrap();
        let hits = names(&net, "nav");
        assert_eq!(hits, ["Navy Yard", "Navy Yard East"], "shorter prefix match first");
    }

    #[test]
    fn gives_nothing_for_nothing() {
        let net = parse(MAP).unwrap();
        assert!(names(&net, "").is_empty());
        assert!(names(&net, "   ").is_empty());
        assert!(names(&net, "nowhere").is_empty());
        assert!(net.search("navy", 0).is_empty());
    }

    #[test]
    fn honours_the_limit_and_skips_unnamed_stations() {
        let net = parse(MAP).unwrap();
        assert_eq!(net.search("a", 2).len(), 2);
        assert_eq!(net.stations.iter().filter(|s| !s.is_searchable()).count(), 1);
        for id in net.search("", 8).iter().chain(net.search("a", 8).iter()) {
            assert!(net.station(*id).is_searchable());
        }
    }

    #[test]
    fn resolves_a_typed_name_back_to_one_station() {
        let net = parse(MAP).unwrap();
        assert_eq!(net.station_named("Navy Yard"), net.search("Navy Yard", 1).first().copied());
        assert_eq!(net.station_named("zizkov"), net.search("Žižkov", 1).first().copied());
        assert_eq!(net.station_named("nope"), None);
    }
}
