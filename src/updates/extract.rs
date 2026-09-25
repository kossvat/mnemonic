//! The pass over tokens that turns them into values and identifying words.

use super::super::lexicon as lx;
use super::{Class, Scan, Tok, Value, amount};

/// How many tokens back a predicate still anchors a bare number or date.
const ANCHOR_WINDOW: usize = 3;

pub(super) struct Extractor<'a> {
    tokens: &'a [Tok],
    /// Sentence index of each token, and whether that sentence talks business.
    commercial: Vec<bool>,
    /// Whether each sentence asks, proposes or bounds rather than states.
    hedged: Vec<bool>,
    sentence_of: Vec<usize>,
    scan: Scan,
    /// Position of a "was" / "from" cue: the value right after it is prior.
    prior_at: Option<usize>,
    /// Position of a "not": the value right after it is denied.
    denied_at: Option<usize>,
    /// Position of a bound word ("under", "up to"): the value after it is
    /// not exact.
    bound_at: Option<usize>,
    /// Identifying words since the last value or sentence start.
    clause: Vec<String>,
    /// Token index where the value being read begins: a cue right before
    /// it ("not", "was") applies to the whole value, however many tokens
    /// it spans ("not Net 60").
    value_start: usize,
    in_reason: bool,
    /// Position (in word and number tokens) of the last predicate, and which.
    last_pred: Option<(usize, &'static str)>,
    last_date_pred: Option<usize>,
    position: usize,
}

impl<'a> Extractor<'a> {
    pub(super) fn new(tokens: &'a [Tok]) -> Self {
        let mut sentence_of = Vec::with_capacity(tokens.len());
        let mut commercial = vec![false];
        // A sentence that asks, proposes or bounds its value.
        let mut hedged = vec![false];
        for token in tokens {
            sentence_of.push(commercial.len() - 1);
            let last = commercial.len() - 1;
            match token {
                Tok::Stop => {
                    commercial.push(false);
                    hedged.push(false);
                }
                Tok::Ask => {
                    hedged[last] = true;
                    commercial.push(false);
                    hedged.push(false);
                }
                Tok::Op => hedged[last] = true,
                Tok::Word(word) => {
                    commercial[last] |= lx::commercial(word);
                    hedged[last] |= lx::modal(word);
                }
                _ => {}
            }
        }
        Self {
            tokens,
            commercial,
            hedged,
            sentence_of,
            scan: Scan::default(),
            prior_at: None,
            denied_at: None,
            bound_at: None,
            clause: Vec::new(),
            value_start: 0,
            in_reason: false,
            last_pred: None,
            last_date_pred: None,
            position: 0,
        }
    }

    pub(super) fn run(mut self) -> Scan {
        let mut i = 0;
        while i < self.tokens.len() {
            i = self.step(i);
        }
        self.scan
    }

    fn word(&self, i: usize) -> Option<&str> {
        match self.tokens.get(i) {
            Some(Tok::Word(word)) => Some(word),
            _ => None,
        }
    }

    fn num(&self, i: usize) -> Option<&str> {
        match self.tokens.get(i) {
            Some(Tok::Num(number)) => Some(number),
            _ => None,
        }
    }

    /// A date is specific enough that a date predicate anywhere earlier in
    /// its sentence anchors it: "deadline for the pilot is 2026-10-15".
    fn date_anchored(&self) -> bool {
        self.last_date_pred.is_some()
    }

    fn push(&mut self, class: Class, key: String, surface: String) {
        let start = self.value_start;
        let right_after = |at: Option<usize>| at.is_some_and(|at| start == at + 1);
        // Words between a cue and the value that only color it, or name
        // what the value is ("does not cost $6").
        let ignorable = |from: usize| {
            (from + 1..start).all(|k| {
                matches!(&self.tokens[k], Tok::Word(w)
                    if lx::change_marker(w) || lx::stopword(w) || lx::predicate(w).is_some())
            })
        };
        // A denial reaches past those ("not currently $6"); "was" does not
        // ("was raised to $6" states the new value).
        let denied = self.denied_at.is_some_and(|at| at < start && ignorable(at));
        let prior = right_after(self.prior_at);
        if self
            .bound_at
            .take()
            .is_some_and(|at| at < start && ignorable(at))
        {
            self.scan.hypothetical = true;
        }
        // What the value is about: the words since the last stated value. A
        // prior value ("from $5 to $6") shares them with the value it names.
        let mut context = if prior {
            self.clause.clone()
        } else {
            std::mem::take(&mut self.clause)
        };
        context.sort_unstable();
        context.dedup();
        let value = Value {
            class,
            key,
            surface,
            pred: self.last_pred.map(|(_, pred)| pred),
            context,
        };
        self.prior_at = None;
        self.denied_at = None;
        // "price is not $6" states no price at all.
        if denied {
            return;
        }
        if self.hedged[self.sentence_of[start.min(self.tokens.len() - 1)]] {
            self.scan.hypothetical = true;
        }
        // A mention said twice (a title repeating the first line) is one
        // value; the same amount for something else is another.
        let list = if prior {
            &mut self.scan.prior
        } else {
            &mut self.scan.values
        };
        let repeated = list
            .iter()
            .any(|v| v.key == value.key && v.pred == value.pred && v.context == value.context);
        if !repeated {
            list.push(value);
        }
    }

    /// Handle the token at `i`; return the index of the next unread token.
    fn step(&mut self, i: usize) -> usize {
        match &self.tokens[i] {
            Tok::Stop | Tok::Ask => {
                self.prior_at = None;
                self.denied_at = None;
                self.bound_at = None;
                self.clause.clear();
                self.in_reason = false;
                self.last_pred = None;
                self.last_date_pred = None;
                i + 1
            }
            Tok::Cur(code, symbol) => {
                let code = *code;
                let surface = symbol.to_string();
                self.value_start = self.signed_start(i);
                match self.num(i + 1) {
                    Some(_) => self.money_after_symbol(i + 1, code, surface),
                    None => i + 1,
                }
            }
            Tok::Num(_) => self.number(i),
            Tok::Date(date) => {
                let date = date.clone();
                self.value_start = i;
                self.position += 1;
                if self.date_anchored() {
                    self.push(Class::Date, format!("date:{date}"), date);
                }
                i + 1
            }
            Tok::Word(word) => {
                let word = word.clone();
                self.word_token(i, &word)
            }
            Tok::Ident(ident) => {
                if !self.in_reason {
                    self.clause.push(ident.clone());
                    self.scan.words.insert(ident.clone());
                }
                i + 1
            }
            Tok::Pct | Tok::Slash | Tok::Minus | Tok::Op => i + 1,
        }
    }

    fn word_token(&mut self, i: usize, word: &str) -> usize {
        self.position += 1;
        if lx::negation(word) {
            self.denied_at = Some(i);
            return i + 1;
        }
        // "under $6", "up to $6", "at least $6": checked before stopwords,
        // which several of these words are.
        if lx::bound(word) || (word == "up" && self.word(i + 1) == Some("to")) {
            self.bound_at = Some(if word == "up" { i + 1 } else { i });
            return i + 1;
        }
        // Only the value right after the cue: "was $5" names the old
        // value, "was raised to $6" states the new one.
        if lx::prior_cue(word) {
            self.prior_at = Some(i);
            return i + 1;
        }
        if word == "instead" && self.word(i + 1) == Some("of") {
            self.position += 1;
            self.prior_at = Some(i + 1);
            return i + 2;
        }
        // "from $5 to $6", "с 5 до 6": the first value is the old one.
        if matches!(word, "from" | "с" | "со" | "от")
            && matches!(self.tokens.get(i + 1), Some(Tok::Cur(..) | Tok::Num(_)))
        {
            self.prior_at = Some(i);
            return i + 1;
        }
        if word == "net"
            && let Some(days) = self.num(i + 1)
            && days.len() <= 3
            && days.chars().all(|c| c.is_ascii_digit())
        {
            let days = days.to_owned();
            self.value_start = i;
            self.position += 1;
            self.push(Class::Terms, format!("net:{days}"), format!("net {days}"));
            return i + 2;
        }
        if let Some(next) = self.month_date(i, word) {
            return next;
        }
        // `USD 6/month`: a currency code before the amount, like a symbol.
        if let Some(code) = lx::currency(word)
            && self.num(i + 1).is_some()
        {
            // The code and its amount are one value, as `$6` is.
            self.position -= 1;
            self.value_start = self.signed_start(i);
            return self.money_after_symbol(i + 1, code, format!("{word} "));
        }
        if lx::reason_cue(word) {
            self.in_reason = true;
            return i + 1;
        }
        if lx::date_predicate(word) {
            self.last_date_pred = Some(self.position);
        }
        if let Some(pred) = lx::predicate(word) {
            self.last_pred = Some((self.position, pred));
            if !self.in_reason {
                self.scan.preds.insert(pred);
            }
            return i + 1;
        }
        if self.in_reason
            || lx::stopword(word)
            || lx::change_marker(word)
            || lx::currency(word).is_some()
        {
            return i + 1;
        }
        let stem = stem(word);
        self.clause.push(stem.clone());
        self.scan.words.insert(stem);
        i + 1
    }

    /// `october 15 2026`, `15 октября` after a date predicate.
    fn month_date(&mut self, i: usize, word: &str) -> Option<usize> {
        let month = lx::month(word)?;
        let day: u8 = self.num(i + 1)?.parse().ok()?;
        self.value_start = i;
        if !(1..=31).contains(&day) || !self.date_anchored() {
            return None;
        }
        let year = self
            .num(i + 2)
            .filter(|y| y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()))
            .map(str::to_owned);
        let key = match &year {
            Some(year) => format!("date:{year}-{month:02}-{day:02}"),
            None => format!("date:{month:02}-{day:02}"),
        };
        let surface = match &year {
            Some(year) => format!("{word} {day} {year}"),
            None => format!("{word} {day}"),
        };
        // The month word itself was counted by the caller.
        self.position += 1 + usize::from(year.is_some());
        self.push(Class::Date, key, surface);
        Some(i + 2 + usize::from(year.is_some()))
    }

    /// A billing period at `i`: `month` after `/`, `per` or `в`.
    fn period_at(&self, i: usize) -> Option<&'static str> {
        self.word(i).and_then(lx::period)
    }

    /// Optional multiplier, then optional `/month` or `per month`, from `i`.
    fn suffixes(&self, mut i: usize) -> (u32, Option<&'static str>, String, usize) {
        let mut factor = 0;
        let mut text = String::new();
        if let Some(word) = self.word(i)
            && let Some(m) = lx::multiplier(word)
        {
            factor = m;
            text.push_str(word);
            i += 1;
        }
        let mut period = None;
        let joiner = match self.tokens.get(i) {
            Some(Tok::Slash) => Some("/"),
            Some(Tok::Word(w)) if matches!(w.as_str(), "per" | "a" | "в" | "за") => {
                Some(" per ")
            }
            _ => None,
        };
        if let Some(joiner) = joiner
            && let Some(p) = self.period_at(i + 1)
        {
            period = Some(p);
            text.push_str(joiner.trim_end());
            if joiner != "/" {
                text.push(' ');
            }
            text.push_str(self.word(i + 1).unwrap_or(p));
            i += 2;
        }
        (factor, period, text, i)
    }

    fn money_key(code: &str, value: &str, period: Option<&str>) -> String {
        match period {
            Some(period) => format!("{code}:{value}/{period}"),
            None => format!("{code}:{value}"),
        }
    }

    /// Where a value starting at `i` really starts: at its sign, if any.
    fn signed_start(&self, i: usize) -> usize {
        if i > 0 && self.tokens[i - 1] == Tok::Minus {
            i - 1
        } else {
            i
        }
    }

    fn negative(&self) -> bool {
        self.tokens.get(self.value_start) == Some(&Tok::Minus)
    }

    fn money_after_symbol(&mut self, i: usize, code: &'static str, mut surface: String) -> usize {
        let (mut raw, next) = self.joined_number(i, true);
        if self.negative() {
            raw.insert(0, '-');
        }
        let (factor, period, suffix, next) = self.suffixes(next);
        self.position += 1;
        let Some(value) = amount(&raw, factor) else {
            return next;
        };
        surface.push_str(&raw);
        surface.push_str(&suffix);
        self.push(Class::Money, Self::money_key(code, &value, period), surface);
        next
    }

    /// A number at `i`, with `5 000` style groups joined when money follows
    /// (or `after_symbol`). Returns the raw digits and the next index.
    fn joined_number(&self, i: usize, after_symbol: bool) -> (String, usize) {
        let mut raw = self.num(i).unwrap_or_default().to_owned();
        let mut j = i + 1;
        let mut groups = Vec::new();
        while let Some(group) = self.num(j)
            && group.len() == 3
            && group.chars().all(|c| c.is_ascii_digit())
        {
            groups.push(group);
            j += 1;
        }
        let currency_follows = self.currency_at(j).is_some()
            || self
                .word(j)
                .is_some_and(|w| lx::multiplier(w).is_some() && self.currency_at(j + 1).is_some());
        if !groups.is_empty() && (after_symbol || currency_follows) {
            for group in groups {
                raw.push_str(group);
            }
            (raw, j)
        } else {
            (raw, i + 1)
        }
    }

    fn currency_at(&self, i: usize) -> Option<(&'static str, String)> {
        match self.tokens.get(i)? {
            Tok::Cur(code, symbol) => Some((code, symbol.to_string())),
            Tok::Word(word) => lx::currency(word).map(|code| (code, word.clone())),
            _ => None,
        }
    }

    fn number(&mut self, i: usize) -> usize {
        let (mut raw, after) = self.joined_number(i, false);
        self.value_start = self.signed_start(i);
        if self.negative() {
            raw.insert(0, '-');
        }
        // `5000 руб`, `5k usd`, `5 тыс руб`
        let (factor, multiplier_text, after_mult) = match self
            .word(after)
            .and_then(|w| lx::multiplier(w).map(|m| (m, w.to_owned())))
        {
            Some((m, w)) if self.currency_at(after + 1).is_some() => (m, w, after + 1),
            _ => (0, String::new(), after),
        };
        if let Some((code, currency)) = self.currency_at(after_mult) {
            let (_, period, suffix, next) = self.suffixes(after_mult + 1);
            self.position += 1;
            if let Some(value) = amount(&raw, factor) {
                let space = if multiplier_text.is_empty() { "" } else { " " };
                let surface = format!("{raw}{space}{multiplier_text} {currency}{suffix}");
                self.push(Class::Money, Self::money_key(code, &value, period), surface);
            }
            return next;
        }
        self.position += 1;
        // `15 октября`, `15 october 2026` after a date predicate.
        if let Some(month) = self.word(i + 1).and_then(lx::month)
            && let Ok(day) = raw.parse::<u8>()
            && (1..=31).contains(&day)
            && self.date_anchored()
        {
            let word = self.word(i + 1).unwrap_or_default().to_owned();
            let year = self
                .num(i + 2)
                .filter(|y| y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()))
                .map(str::to_owned);
            self.position += 1 + usize::from(year.is_some());
            let (key, surface) = match &year {
                Some(year) => (
                    format!("date:{year}-{month:02}-{day:02}"),
                    format!("{day} {word} {year}"),
                ),
                None => (format!("date:{month:02}-{day:02}"), format!("{day} {word}")),
            };
            self.push(Class::Date, key, surface);
            return i + 2 + usize::from(year.is_some());
        }
        let sentence = self.sentence_of[i];
        let percent_word = self
            .word(i + 1)
            .is_some_and(|w| w == "percent" || w.starts_with("процент"));
        if matches!(self.tokens.get(i + 1), Some(Tok::Pct)) || percent_word {
            if self.commercial[sentence]
                && let Some(value) = amount(&raw, 0)
            {
                self.push(Class::Percent, format!("pct:{value}"), format!("{raw}%"));
            }
            return i + 2;
        }
        if let Some((_, pred)) = self
            .last_pred
            .filter(|(at, _)| self.position - at <= ANCHOR_WINDOW)
        {
            let (factor, period, suffix, next) = self.suffixes(after);
            if let Some(value) = amount(&raw, factor) {
                // `10 per day` and `10 per year` are two limits.
                let key = match period {
                    Some(period) => format!("num:{pred}:{value}/{period}"),
                    None => format!("num:{pred}:{value}"),
                };
                self.push(Class::Number, key, format!("{raw}{suffix}"));
            }
            return next;
        }
        // A number that is no value still says which thing this is:
        // "Widget 1" is not "Widget 2", "per 100 units" not "per 1000".
        if !self.in_reason {
            let word = format!("n:{raw}");
            self.clause.push(word.clone());
            self.scan.words.insert(word);
        }
        after
    }
}

/// What identifies a word across plural and case endings, roughly. The
/// parts of a compound are kept apart (`widget-basic` is not
/// `widget-premium`), and nothing but an ending is ever cut, so two names
/// that merely begin alike stay two.
fn stem(word: &str) -> String {
    let word = word
        .trim_end_matches("'s")
        .trim_end_matches("’s")
        .trim_matches(['-', '\'']);
    word.split('-')
        .filter(|part| !part.is_empty())
        .map(stem_part)
        .collect::<Vec<_>>()
        .join("-")
}

fn stem_part(part: &str) -> String {
    const RU_ENDINGS: &[&str] = &[
        "ами", "ями", "ого", "его", "ому", "ему", "ой", "ей", "ом", "ем", "ам", "ям", "ах", "ях",
        "ов", "ев", "ы", "и", "а", "я", "у", "ю", "е", "о", "ь",
    ];
    let letters = part.chars().count();
    let cyrillic = part.chars().any(|c| matches!(c, 'а'..='я' | 'ё'));
    if cyrillic {
        RU_ENDINGS
            .iter()
            .find(|ending| part.ends_with(**ending) && letters - ending.chars().count() >= 3)
            .map(|ending| part[..part.len() - ending.len()].to_owned())
            .unwrap_or_else(|| part.to_owned())
    } else if letters > 4 && part.ends_with("ies") {
        format!("{}y", &part[..part.len() - 3])
    } else if letters > 3 && part.ends_with('s') && !part.ends_with("ss") {
        part[..part.len() - 1].to_owned()
    } else {
        part.to_owned()
    }
}
