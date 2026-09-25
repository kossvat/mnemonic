//! Word lists the value scanner reads. All generic, EN and RU.

/// Currency word (lowercased token) or symbol to its ISO-like code.
pub fn currency(word: &str) -> Option<&'static str> {
    Some(match word {
        "$" | "usd" | "dollar" | "dollars" | "бакс" | "баксов" => "usd",
        "€" | "eur" | "euro" | "euros" | "евро" => "eur",
        "£" | "gbp" => "gbp",
        "₽" | "rub" => "rub",
        "uah" | "грн" | "гривен" => "uah",
        // Every case of the word, and nothing else that starts alike
        // (рубашка, рубильник are not money).
        "руб" | "рубль" | "рубля" | "рублю" | "рублем" | "рублём" | "рубле" | "рубли"
        | "рублей" | "рублям" | "рублями" | "рублях" => "rub",
        "долл" | "доллар" | "доллара" | "доллару" | "долларом" | "долларе" | "доллары"
        | "долларов" | "долларам" | "долларами" | "долларах" => {
            "usd"
        }
        _ => return None,
    })
}

/// Multiplier word right after an amount (`5k`, `2 млн`), as a power of ten.
pub fn multiplier(word: &str) -> Option<u32> {
    Some(match word {
        "k" | "к" | "тыс" | "тысяч" | "тысячи" => 3,
        "m" | "mm" | "млн" | "миллион" | "миллиона" | "миллионов" => 6,
        _ => return None,
    })
}

/// Billing period after `per`, `/` or `в`: part of a price's identity.
pub fn period(word: &str) -> Option<&'static str> {
    Some(match word {
        "month" | "mo" | "monthly" | "месяц" | "мес" => "month",
        "year" | "yr" | "annually" | "год" => "year",
        "week" | "wk" | "неделю" | "неделя" => "week",
        "day" | "daily" | "день" | "сутки" => "day",
        "hour" | "hr" | "час" => "hour",
        "user" | "seat" | "пользователя" | "место" => "seat",
        "unit" | "piece" | "pc" | "шт" | "штуку" => "unit",
        _ => return None,
    })
}

/// Predicate words, canonicalised: price = cost = pricing = цена.
pub fn predicate(word: &str) -> Option<&'static str> {
    let exact = match word {
        "price" | "prices" | "pricing" | "priced" | "cost" | "costs" => "price",
        "fee" | "fees" => "fee",
        "commission" | "commissions" => "commission",
        "discount" | "discounts" => "discount",
        "budget" | "budgets" => "budget",
        "limit" | "limits" | "cap" => "limit",
        "quota" | "quotas" => "quota",
        "moq" => "moq",
        "term" | "terms" | "payment" => "terms",
        "rate" | "rates" => "rate",
        "margin" | "margins" => "margin",
        "markup" => "markup",
        "tax" | "taxes" | "vat" => "tax",
        "deadline" | "deadlines" | "due" | "date" | "dates" => "deadline",
        _ => "",
    };
    if !exact.is_empty() {
        return Some(exact);
    }
    const STEMS: &[(&str, &str)] = &[
        ("цен", "price"),
        ("стоим", "price"),
        ("комисс", "commission"),
        ("скидк", "discount"),
        ("бюджет", "budget"),
        ("лимит", "limit"),
        ("квот", "quota"),
        ("услови", "terms"),
        ("оплат", "terms"),
        ("ставк", "rate"),
        ("марж", "margin"),
        ("наценк", "markup"),
        ("налог", "tax"),
        ("ндс", "tax"),
        ("дедлайн", "deadline"),
        ("срок", "deadline"),
    ];
    STEMS
        .iter()
        .find(|(stem, _)| word.starts_with(stem))
        .map(|(_, canonical)| *canonical)
}

/// A bare percentage is a value only in a sentence that talks business.
pub fn commercial(word: &str) -> bool {
    matches!(
        predicate(word),
        Some("commission" | "discount" | "margin" | "markup" | "fee" | "rate" | "tax")
    ) || word.starts_with("royalt")
        || word.starts_with("роялти")
}

/// Words after which a date is a value: `deadline 2026-10-15`, `до 15 октября`.
pub fn date_predicate(word: &str) -> bool {
    matches!(
        word,
        "deadline"
            | "due"
            | "launch"
            | "launches"
            | "release"
            | "until"
            | "effective"
            | "renewal"
            | "renews"
            | "expires"
            | "срок"
            | "дедлайн"
            | "до"
            | "запуск"
            | "релиз"
    )
}

pub fn month(word: &str) -> Option<u8> {
    const EN: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    // RU nominative and genitive share these stems: октябрь, октября.
    const RU: [&str; 12] = [
        "январ",
        "феврал",
        "март",
        "апрел",
        "ма",
        "июн",
        "июл",
        "август",
        "сентябр",
        "октябр",
        "ноябр",
        "декабр",
    ];
    if word.chars().count() >= 3
        && let Some(i) = EN.iter().position(|full| full.starts_with(word))
    {
        return Some(i as u8 + 1);
    }
    if matches!(word, "май" | "мая" | "мае") {
        return Some(5);
    }
    RU.iter()
        .position(|stem| *stem != "ма" && word.starts_with(stem))
        .map(|i| i as u8 + 1)
}

/// Words before a number that make it an identifier, not a value.
pub fn counter_keyword(word: &str) -> bool {
    matches!(
        word,
        "build"
            | "step"
            | "phase"
            | "round"
            | "batch"
            | "pr"
            | "issue"
            | "line"
            | "page"
            | "version"
            | "port"
            | "item"
            | "row"
            | "этап"
            | "шаг"
            | "раунд"
            | "билд"
            | "версия"
            | "порт"
            | "строка"
            | "пункт"
    )
}

/// Unit words after a number that make it a measurement, not a value.
pub fn measure_unit(word: &str) -> bool {
    matches!(
        word,
        "fps"
            | "ms"
            | "s"
            | "sec"
            | "secs"
            | "seconds"
            | "min"
            | "mins"
            | "minutes"
            | "b"
            | "kb"
            | "mb"
            | "gb"
            | "tb"
            | "px"
            | "dpi"
            | "hz"
            | "khz"
            | "mhz"
            | "ghz"
            | "mbps"
            | "gbps"
            | "tokens"
            | "мс"
            | "сек"
            | "мин"
            | "кб"
            | "мб"
            | "гб"
            | "тб"
            | "пикс"
    )
}

/// A value right after one of these is what USED to hold.
pub fn prior_cue(word: &str) -> bool {
    matches!(
        word,
        "was"
            | "were"
            | "previously"
            | "formerly"
            | "было"
            | "был"
            | "была"
            | "были"
            | "раньше"
            | "ранее"
            | "вместо"
            | "прежняя"
            | "прежний"
    )
}

/// A sentence with one of these proposes, asks or supposes a value; it
/// does not state one.
pub fn modal(word: &str) -> bool {
    matches!(
        word,
        "should"
            | "could"
            | "would"
            | "might"
            | "maybe"
            | "perhaps"
            | "if"
            | "whether"
            | "suppose"
            | "propose"
            | "proposed"
            | "proposal"
            | "suggest"
            | "suggested"
            | "consider"
            | "considering"
            | "может"
            | "можно"
            | "возможно"
            | "если"
            | "стоит"
            | "предлагаю"
            | "предложить"
            | "предложение"
            | "давай"
            | "давайте"
            | "наверное"
            | "вероятно"
    )
}

/// A value right after one of these is a bound or an estimate, not an
/// exact figure: "under $6", "up to", "at least", "не более".
pub fn bound(word: &str) -> bool {
    matches!(
        word,
        "under"
            | "over"
            | "below"
            | "above"
            | "max"
            | "maximum"
            | "min"
            | "minimum"
            | "approximately"
            | "approx"
            | "around"
            | "roughly"
            | "about"
            | "least"
            | "most"
            | "than"
            | "upto"
            | "свыше"
            | "более"
            | "менее"
            | "около"
            | "примерно"
            | "порядка"
            | "максимум"
            | "минимум"
    )
}

/// A value right after one of these is denied, not stated.
pub fn negation(word: &str) -> bool {
    matches!(
        word,
        "not" | "isn't" | "isn’t" | "never" | "не" | "нет" | "ни"
    )
}

/// Words after one of these in a sentence explain, they do not identify.
pub fn reason_cue(word: &str) -> bool {
    matches!(
        word,
        "because" | "since" | "according" | "из-за" | "потому" | "согласно" | "так"
    )
}

/// Words that say something changed but not what: dropped from identity.
pub fn change_marker(word: &str) -> bool {
    matches!(
        word,
        "now"
            | "new"
            | "updated"
            | "update"
            | "updates"
            | "changed"
            | "change"
            | "changes"
            | "raised"
            | "lowered"
            | "increased"
            | "decreased"
            | "dropped"
            | "bumped"
            | "went"
            | "goes"
            | "moved"
            | "shifted"
            | "перенесли"
            | "сдвинули"
            | "became"
            | "becomes"
            | "back"
            | "again"
            | "currently"
            | "current"
            | "actually"
            | "correction"
            | "corrected"
            | "replaces"
            | "replaced"
            | "replace"
            | "earlier"
            | "previous"
            | "fact"
            | "value"
            | "today"
            | "fyi"
            | "note"
            | "теперь"
            | "сейчас"
            | "новая"
            | "новый"
            | "новое"
            | "новые"
            | "обновили"
            | "обновил"
            | "обновление"
            | "изменилась"
            | "изменился"
            | "изменили"
            | "поменялась"
            | "поменяли"
            | "подняли"
            | "снизили"
            | "повысили"
            | "понизили"
            | "снова"
            | "опять"
            | "уже"
            | "стала"
            | "стал"
            | "стало"
            | "итог"
            | "кстати"
            | "поправка"
    )
}

/// Words that carry nothing about what a value belongs to. Not here: the
/// pricing basis ("each", "all", "every", "все", "before", "after" tax),
/// which does.
pub fn stopword(word: &str) -> bool {
    matches!(
        word,
        "a" | "an"
            | "the"
            | "is"
            | "are"
            | "was"
            | "were"
            | "be"
            | "been"
            | "being"
            | "am"
            | "of"
            | "to"
            | "in"
            | "on"
            | "at"
            | "by"
            | "for"
            | "with"
            | "from"
            | "into"
            | "and"
            | "or"
            | "but"
            | "not"
            | "no"
            | "yes"
            | "it"
            | "its"
            | "this"
            | "that"
            | "these"
            | "those"
            | "as"
            | "so"
            | "our"
            | "we"
            | "i"
            | "you"
            | "they"
            | "he"
            | "she"
            | "my"
            | "your"
            | "their"
            | "there"
            | "here"
            | "has"
            | "have"
            | "had"
            | "will"
            | "would"
            | "should"
            | "can"
            | "could"
            | "do"
            | "does"
            | "did"
            | "per"
            | "set"
            | "up"
            | "down"
            | "about"
            | "than"
            | "then"
            | "also"
            | "just"
            | "only"
            | "any"
            | "some"
            | "which"
            | "what"
            | "who"
            | "whom"
            | "how"
            | "when"
            | "where"
            | "why"
            | "if"
            | "else"
            | "over"
            | "under"
            | "until"
            | "via"
            | "instead"
            | "и"
            | "в"
            | "во"
            | "на"
            | "с"
            | "со"
            | "по"
            | "к"
            | "ко"
            | "о"
            | "об"
            | "от"
            | "до"
            | "за"
            | "из"
            | "у"
            | "не"
            | "но"
            | "а"
            | "же"
            | "ли"
            | "это"
            | "эта"
            | "этот"
            | "эти"
            | "как"
            | "что"
            | "то"
            | "для"
            | "при"
            | "мы"
            | "я"
            | "ты"
            | "вы"
            | "он"
            | "она"
            | "они"
            | "наш"
            | "наша"
            | "наше"
            | "наши"
            | "его"
            | "её"
            | "ее"
            | "их"
            | "есть"
            | "будет"
            | "будут"
            | "был"
            | "была"
            | "были"
            | "было"
            | "или"
            | "также"
            | "только"
            | "там"
            | "тут"
            | "здесь"
            | "бы"
            | "да"
            | "нет"
            | "тоже"
            | "чем"
            | "где"
            | "когда"
            | "если"
            | "про"
            | "над"
            | "под"
            | "через"
            | "после"
            | "перед"
            | "между"
    )
}
