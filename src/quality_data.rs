//! The words the rules of [`crate::quality`] look for, and the turns they
//! are tested on.
//!
//! Every sentence here is invented. All text of this kind lives in this one
//! file, so that it can be read in one place.

/// How a redaction marker opens. What follows it up to the closing bracket
/// names what was removed.
pub const REDACTION_MARKER_OPEN: &str = "[REDACTED:";

/// A correction is brief: a longer reply is not one.
pub const CORRECTION_MAX_BYTES: usize = 500;

/// A weak marker counts only near the start of a reply.
pub const CORRECTION_HEAD_CHARS: usize = 50;

/// How many fronts are taken off a line before its shape is left open.
pub const SHAPE_ROUNDS: usize = 8;

/// What a list, a quote, a heading, a difference or bold puts in front of
/// a line.
pub const LINE_MARKS: &[char] = &['#', '-', '*', '_', '>', '+', '•'];

/// The bars between the cells of a table, typed or drawn.
pub const BARS: &[char] = &['|', '│', '┃'];

/// The words of data that are written bare, without quotes.
pub const WORDS_OF_DATA: &[&str] = &["true", "false", "null"];

/// What ends a clause: the marks of a sentence, a comma and a semicolon.
pub const CLAUSE_ENDS: &[char] = &['.', '!', '?', ',', ';'];

/// What may stand between the mark that ends a clause and the space
/// behind it.
pub const CLOSING_MARKS: &[char] = &['*', '_', '`', ')', ']', '"', '\'', '»', '”', '’'];

/// Apostrophes as a typesetter writes them. The lists have the one of the
/// keyboard.
pub const APOSTROPHES: &[char] = &['\u{2019}', '\u{02bc}', '\u{2018}'];

/// Characters that stand inside a word without showing: the soft hyphen and
/// the two joiners.
pub const UNSEEN_IN_WORDS: &[char] = &['\u{ad}', '\u{200c}', '\u{200d}'];

/// Characters that join two words into one name: before a word always,
/// behind a word when a word follows them. A slash joins whatever it
/// touches: it makes a path.
pub const JOINERS: &[char] = &['-', '_', '.'];

/// Particles that a hyphen glues to a word without making a longer word.
pub const PARTICLES: &[&str] = &["ка"];

/// The blocks of accents written as characters of their own after their
/// letter: combining diacritical marks, their extended and supplement
/// blocks, the marks for symbols, and the half marks.
pub const ACCENTS: &[std::ops::RangeInclusive<char>] = &[
    '\u{300}'..='\u{36f}',
    '\u{1ab0}'..='\u{1aff}',
    '\u{1dc0}'..='\u{1dff}',
    '\u{20d0}'..='\u{20ff}',
    '\u{fe20}'..='\u{fe2f}',
];

/// Markers that make a correction wherever they stand.
///
/// As a word a marker no longer reaches the forms that begin with it, so
/// the forms of everyday speech are markers of their own.
pub const CORRECTION_STRONG: &[&str] = &[
    "не так",
    "не такой",
    "не такая",
    "не такое",
    "не такие",
    "не такую",
    "не таким",
    "не такого",
    "не таких",
    "не такому",
    "не такими",
    "не то",
    "не тот",
    "не та",
    "не те",
    "не того",
    "не той",
    "не тому",
    "не туда",
    "не подходит",
    "стоп",
    "стоп-стоп",
    "stop",
    "нет,",
    "нет-нет,",
    "nope",
    "переделай",
    "переделайте",
    "redo",
    "revert",
    "be reverted",
    "reverting",
    "откатить",
    "верни",
    "верните",
    "вернись",
    "вернитесь",
    "не делай",
    "не делайте",
    "я имел в виду",
    "i meant",
    "это неправильно",
    "это неправильное",
    "that's wrong",
    "that is wrong",
    "ignore that",
];

/// Phrases of everyday speech that hold a strong marker and correct
/// nothing. A marker inside one of them does not count. A phrase stands
/// as a word, so "не так уж" is not in "не так ужимай".
pub const CORRECTION_IDIOMS: &[&str] = &["не так давно", "не так ли", "не то чтобы", "если нет,"];

/// Markers that a negation turns into a reminder.
pub const CORRECTION_NEGATABLE: &[&str] = &["забудь", "забудьте", "forget"];

/// The negations that do so. They refuse by their letters, as capture has
/// it, so the first of them refuses its longer forms too.
pub const CORRECTION_NEGATED: &[&str] = &["не забудь", "don't forget", "dont forget", "not forget"];

/// Markers that also live in plain requests: counted only near the start.
pub const CORRECTION_WEAK: &[&str] = &["не надо", "don't", "dont", "wrong", "instead"];

/// Phrases that make a decision.
pub const DECISION: &[&str] = &[
    "давай используем",
    "let's use",
    "lets use",
    "выбираем",
    "we'll go with",
    "going with",
    "решение:",
    "decision:",
    "decided",
    "используем",
    "переиспользуем",
    "будем использовать",
    "архитектура:",
    "architecture:",
    "стек:",
    "техстек:",
    "stack:",
    "techstack:",
    "переходим на",
    "switching to",
    "migrating to",
];

/// What the strict lists hold that capture does not look for. Capture
/// reaches most of them through a marker they begin with. The others keep a
/// reply that capture took by another marker.
#[cfg_attr(not(test), allow(dead_code))]
pub const ADDED_STRONG: &[&str] = &[
    "не такой",
    "не такая",
    "не такое",
    "не такие",
    "не такую",
    "не таким",
    "не такого",
    "не таких",
    "не такому",
    "не такими",
    "не тот",
    "не та",
    "не те",
    "не того",
    "не той",
    "не тому",
    "стоп-стоп",
    "нет-нет,",
    "переделайте",
    "be reverted",
    "reverting",
    "верните",
    "вернись",
    "вернитесь",
    "не делайте",
    "это неправильное",
];
#[cfg_attr(not(test), allow(dead_code))]
pub const ADDED_NEGATABLE: &[&str] = &["забудьте"];
#[cfg_attr(not(test), allow(dead_code))]
pub const ADDED_DECISION: &[&str] = &["переиспользуем", "техстек:", "techstack:"];

#[cfg(test)]
pub mod fixtures {
    use crate::quality::{Capture, Noise};

    /// One reply of a conversation.
    #[derive(Debug, Clone, Copy)]
    pub struct Turn {
        pub role: &'static str,
        pub text: &'static str,
    }

    const fn user(text: &'static str) -> Turn {
        Turn { role: "user", text }
    }

    const fn assistant(text: &'static str) -> Turn {
        Turn {
            role: "assistant",
            text,
        }
    }

    /// Whether a text holds a letter of the Cyrillic script.
    pub fn is_cyrillic(text: &str) -> bool {
        text.chars().any(|c| ('\u{400}'..='\u{4ff}').contains(&c))
    }

    /// Statements. Capture stores each, and no rule may name it. Among them
    /// is every text the tests of detection hold as a decision or a
    /// correction.
    pub const KEPT: &[Turn] = &[
        // Corrections the tests of detection hold.
        user("не так, переделай авторизацию"),
        user("Stop, that's wrong"),
        user("нет, лучше используй PostgreSQL"),
        user("Результат не тот, которого я ожидал: из схемы пропали два индекса."),
        user("Обновилось не то поле, должен меняться updated_at."),
        user("Не делай отдельный класс под каждый формат."),
        user("Не надо кэшировать ответы с ошибкой."),
        user("Не надо трогать публичный API в этом PR."),
        user("Предыдущий черновик схемы забудь: актуальная версия лежит в ветке main."),
        user("Ссылка в README ведёт не туда."),
        user("Откуда взялась эта версия? Формат даты не подходит для API."),
        user("не так, переделай"),
        user("stop, use the other configuration for demoapp"),
        user("не так, use the other bucket"),
        user("не так, use the staging bucket"),
        user("это не то, AGENTS.md чужого проекта"),
        user("это не то, домен меняем на demo.example"),
        user("это не то, живая правка про demoapp"),
        user("это не то, живой захват"),
        user("это не то, запись без папки"),
        user("это не то, запуск из exec"),
        user("это не то, инструкции чужого проекта"),
        user("это не то, общий вопрос по монорепо"),
        user("это не то, первая версия файла"),
        user("это не то, после перезапуска"),
        user("это не то, правка из июля про другое"),
        user("это не то, решение по DemoApp"),
        user("это не то, секрет другого проекта"),
        user("это не то, секрет из монорепо"),
        user("это не то, секрет после переезда"),
        user("это не то, секрет чужого проекта"),
        user("это не то, снова в DemoApp"),
        user("это не то, тесты гоняем на CI"),
        user("это не то, цену ставим 42"),
        user("это не то, цену ставим 43"),
        user("это не то, чужое"),
        user("это не то, чужой секрет"),
        user("это не то, эндпоинт переименуем"),
        // Corrections in the forms of everyday speech.
        user("Формат не такой, как в спецификации: нужна дата ISO."),
        user("Схема не такая, как в документации, поправь поля."),
        user("Поле не такое, как в спецификации: нужен текст."),
        user("Отступы не такие, как в макете, поправь."),
        user("Ты взял не такую библиотеку, нужна асинхронная."),
        user("Не таким путём, сначала миграция."),
        user("Ты поправил не того клиента, нужен demoapp."),
        user("Заголовок не той таблицы, нужна orders."),
        user("Письмо ушло не тому адресату, исправь шаблон."),
        user("Поле не того типа, должно быть число."),
        user("Это неправильное значение, должно быть 42."),
        user("Переделайте заголовок, он слишком длинный."),
        user("Верните прежний цвет кнопки."),
        user("Вернись к предыдущему формату дат."),
        user("Вернитесь к прежнему названию кнопки."),
        user("Не делайте отдельную таблицу под каждый отчёт."),
        user("The change must be reverted."),
        user("This needs to be reverted; keep the old schema."),
        user("The last change needs reverting; restore the old schema."),
        user("Не такого ответа я ждал: в JSON нет поля total."),
        user("Не таких колонок я просил, нужны только даты."),
        user("Не такому клиенту письмо, нужен demoapp."),
        user("Не такими шагами, сначала миграция."),
        // Everyday words that are a correction all the same.
        user("Да нет, оставь старый формат."),
        user("Да нет, я про другой файл."),
        user("Не так ужимай JPEG: сохрани пропорции."),
        user("Ты посчитал не так лимиты по API, они в байтах."),
        user("Это не то что я просил, в отчёте нужны только даты."),
        user("Не то что нужно, смотри макет."),
        user("Нет-нет, оставь как было."),
        user("Стоп-стоп, я не это просил."),
        user("Верни-ка старый цвет кнопки."),
        user("Переделай-ка заголовок, он длинный."),
        user("Переделайте-ка заголовок, он слишком длинный."),
        user("Не так много текста, сократи до одного абзаца."),
        user("Не так уж важно, пропусти этот шаг."),
        // A correction outside an element, with the element behind it.
        user("Стоп, оставь прежнюю схему.\n<example>\nfield=old\n</example>"),
        user("<example>\nfield=old\n</example>\nСтоп, оставь прежнюю схему."),
        // A numbered answer that leads with its refusal.
        user("2. Не надо, оставь как есть"),
        user("2. Don't rename the column"),
        // The key the agent got wrong, and what is right.
        user("\"updated_at\": не то поле, нужно created_at"),
        user("I meant the staging cluster, not production."),
        user("Forget the second option, keep the first."),
        user("Забудьте про старый макет, работаем по новому."),
        user("Instead of a new table, add a column."),
        user("Stop - use the other parser."),
        // A correction next to what was pasted into the reply.
        user("Context first.\nThen: stop, use the other parser for demoapp."),
        user("Don't rename the column.\n1. stop the import\n2. run the tests"),
        user("Don't touch the config, the run printed:\n| stop | reason |"),
        user("Не надо трогать конфиг. Вывод был такой:\n{\"event\": \"stop\"}"),
        user("Вот что вышло:\n| поле | стоп |\nне так, переделай колонку"),
        user("<ide_opened_file>src/stop_words.rs</ide_opened_file>\nне так, переделай авторизацию"),
        user("| don't | touch |\nstop the import, use the other one"),
        // A decision above a pasted row with a marker in it.
        user("Decision: keep SQLite.\n| stop | reason |"),
        user("Решение: используем SQLite.\n| стоп | причина |"),
        // A statement behind what stands in front of it.
        user("<Header> не так выровнен, логотип должен быть слева."),
        user("[README](README.md) ведёт не туда, поправь ссылку."),
        user("1. Не так, заголовок должен быть слева\n2. Верни старый цвет"),
        user("1. Decision: stop caching errors."),
        assistant("1. Stop caching errors; we decided to keep PostgreSQL."),
        assistant("<Header> Decision: use buffers < 64 KiB."),
        assistant("<Header data-note=\"> 1. staged\"> We decided to keep SQLite."),
        assistant("Decision: use ``a | b | c`` as the literal template."),
        assistant("[decision: PostgreSQL] {pool_size: 8}"),
        assistant("Переиспользуем существующую таблицу sessions."),
        assistant("[[ADR-1]] Decision: use SQLite. See [[ADR-2]]"),
        // A decision in a reply capture took for a correction.
        user("The build stopped; let's use SQLite."),
        user("Мне надо поправить конфиг: давай используем SQLite."),
        // Decisions the tests of detection hold.
        user("давай используем PostgreSQL"),
        user("Let's use Redis for caching"),
        user("Переходим на FastAPI"),
        assistant("We'll go with PostgreSQL for the schema layer."),
        assistant("We'll go with Redis for caching"),
        assistant("Switching to isolated queues for unrelated workloads"),
        assistant(
            "Decision: use SQLite for demoapp because of reason number 7 which is long enough to push this file past the prefix window",
        ),
        assistant("Switching to async tokio handles the connection pooling cleanly."),
        assistant("decision: ship the heuristic summarizer first, LLM behind a flag."),
        assistant(
            "Готово.\n\nРешение:\n- используем PostgreSQL\n- SQLite оставляем только для локальных тестов",
        ),
        assistant("Decision: используем 🚀 for demoapp"),
        assistant("Decision: Gadget price is $9"),
        assistant("Decision: Widget price is $5"),
        assistant("Decision: Widget price is now $6"),
        assistant("Decision: go with the smaller parser"),
        assistant("Decision: skip historic turn"),
        assistant("Decision: use PostgreSQL and a much longer configuration"),
        assistant("Decision: use Postgres for the ledger"),
        assistant("Decision: use Postgres"),
        assistant("Decision: use Redis for the cache"),
        assistant("Decision: use Redis!"),
        assistant("Decision: use Redis"),
        assistant("Decision: use SQLite for demoapp"),
        assistant("Decision: use SQLite for the index"),
        assistant("Decision: use SQLite"),
        assistant("Decision: use a considerably longer original configuration"),
        assistant("Decision: use a substantially longer replacement configuration"),
        assistant("<private>the numbers</private>\nDecision: use SQLite for the index"),
        // Decisions in other clothes.
        assistant("Done.\nDecision: keep the smaller index."),
        assistant("**Decision:** keep both caches."),
        assistant("- Stack: Rust, SQLite, Swift"),
        assistant("Техстек: Rust, SQLite, Swift."),
        assistant("Techstack: Rust, SQLite, Swift."),
        assistant("Архитектура: один процесс, три хранилища."),
        user("Будем использовать один формат дат во всех отчётах."),
        user("Мы выбираем второй вариант макета."),
        assistant("We decided to keep the old index until the release."),
        assistant("Migrating to the new schema happens in two steps."),
        // A decision below a line capture started at by a part of a word.
        assistant("Undecided items first.\nDecision: keep both caches."),
        assistant("Неиспользуемые зависимости убрал.\nРешение: используем один формат дат."),
        // A statement followed by a question.
        user("Переходим на FastAPI. Сколько времени займёт миграция?"),
        assistant("Decision: use SQLite for the index. Shall I start the migration?"),
        user("Давай используем Redis для кэша. Сможешь настроить сегодня?"),
        user("`Decision: use SQLite.` Can you implement it?"),
        user("Давай используем Redis, ок?"),
        user("Переходим на FastAPI, ладно?"),
        assistant("Switching to tokio for the runtime, want me to proceed?"),
        assistant("Going with SQLite for the index, sound good?"),
        user("Let's use Redis here, ok?"),
        // Bars that are operators or pipes, not the walls of cells.
        assistant("Decision: use `a || b` for the fallback."),
        assistant("Switching to `cargo test || true` in CI."),
        user("Используем `grep foo | sort | uniq` для отчёта"),
        // A statement behind a label, a box or a link.
        assistant("- [x] Decision: use SQLite, details in [ADR 4](docs/adr-004.md)"),
        assistant("[backend] We decided to use [the smaller parser](docs/parser.md)."),
        assistant("- [ingress.rs](src/watcher/ingress.rs): we decided to keep the old selector."),
        assistant("## 2. Decision: use SQLite for the index"),
        // A year and a version are not the numbers of steps.
        assistant("2026. We decided to keep the old index."),
        assistant("1.5 it is: we decided to keep the old index."),
        assistant("2026.1) We decided to retain SQLite."),
    ];

    /// Noise that capture stores today, with the rule that names it.
    pub const HIDDEN: &[(Noise, Turn)] = &[
        // A row of a table.
        (
            Noise::TableRow,
            assistant("| Настройка | Что выбираем |\n|---|---|\n| Кэш | Redis |"),
        ),
        (
            Noise::TableRow,
            assistant(
                "| Компонент | Что используем | Для чего |\n|---|---|---|\n| Очередь | NATS | события |",
            ),
        ),
        (
            Noise::TableRow,
            assistant(
                "Сводка ниже.\n| Option | Decided | Owner |\n|---|---|---|\n| cache | yes | team |",
            ),
        ),
        (Noise::TableRow, user("| stack: | rust, sqlite |")),
        (
            Noise::TableRow,
            assistant("| Step | Note |\n| 1 | the parser | switching to it later |"),
        ),
        (Noise::TableRow, user("| stop doing that | now |")),
        (
            Noise::TableRow,
            user("Таблица из отчёта:\n| не так | как было |"),
        ),
        (Noise::TableRow, assistant("Option | Decided | Owner")),
        (Noise::TableRow, assistant("│ Option │ Decided │ Owner │")),
        (Noise::Data, assistant("-\u{a0}{\"decided\": true}")),
        (
            Noise::ListStep,
            assistant("@\"t.md\" 1. we are going with it"),
        ),
        // The first rule that matches names the noise.
        (Noise::TableRow, assistant("| are we going with it? |")),
        (Noise::TableRow, assistant("<b>decided | yes | no</b>")),
        (Noise::TableRow, assistant("\"| Option | Decided |\"")),
        (Noise::Markup, assistant("<b>decided? or not</b>")),
        (Noise::Data, assistant("\"decided\": is it?")),
        (Noise::Link, assistant("[{\"decided\": true}](a.md)")),
        (Noise::ListStep, assistant("3. decided already?")),
        // Markup.
        (
            Noise::Markup,
            assistant("<result>{\"status\":\"decided\",\"items\":3}</result>"),
        ),
        (
            Noise::Markup,
            user("<command-name>deploy</command-name> switching to staging"),
        ),
        (
            Noise::Markup,
            assistant("<details><summary>decided items</summary></details>"),
        ),
        (
            Noise::Markup,
            assistant("</thinking> let's use the smaller model"),
        ),
        (Noise::Markup, assistant("<!-- stack: rust, sqlite -->")),
        (Noise::Markup, assistant("<вывод>решение: см. ниже</вывод>")),
        (
            Noise::Markup,
            user(
                "<local-command-stdout>\nabc1234 Switching to tokio for the runtime\n</local-command-stdout>",
            ),
        ),
        (
            Noise::Markup,
            user("<local-command-stdout>\nDecision: use SQLite\n</local-command-stdout>"),
        ),
        (
            Noise::Markup,
            assistant("<local.command.stdout>\nDecision: use SQLite\n</local.command.stdout>"),
        ),
        (
            Noise::Markup,
            user("<example>\nstop the import\n</example>"),
        ),
        (
            Noise::Markup,
            user("<stdout>\nstop internal\n<stdout>\ndata\n</stdout>\n</stdout>"),
        ),
        (Noise::Data, assistant("\"status\": \"decided\", \"id\": 1")),
        (
            Noise::Markup,
            user("<stdout>stop requested by the runner</stdout>"),
        ),
        (
            Noise::Markup,
            assistant("<?xml version=\"1.0\"?><note>decided</note>"),
        ),
        // A link to an image.
        (
            Noise::ImageLink,
            assistant("![3. Что используем готовое и что разрабатываем](output/pack/03.png)"),
        ),
        (
            Noise::ImageLink,
            assistant("![architecture: overview](docs/overview.png)"),
        ),
        (
            Noise::ImageLink,
            assistant("![Решение: схема очередей](img/queues.png)"),
        ),
        (
            Noise::ImageLink,
            assistant("![stack: layers](img/layers.svg)"),
        ),
        (
            Noise::ImageLink,
            assistant("![decided badge](img/badge.svg)"),
        ),
        // A link.
        (
            Noise::Link,
            assistant("[decision: use SQLite](docs/adr-004.md)"),
        ),
        (Noise::Link, assistant("[Решение: журнал](docs/adr-007.md)")),
        (
            Noise::Link,
            assistant("[Switching to the new API](docs/migration.md) is described here"),
        ),
        (
            Noise::Link,
            assistant("[Архитектура: обзор](docs/architecture.md)"),
        ),
        (Noise::Link, assistant("[decided items](docs/decisions.md)")),
        (Noise::Link, assistant("[Switching to the new API][1]")),
        (Noise::Link, assistant("[[decided items]]")),
        (Noise::Link, assistant("[[Decision: use SQLite]] [[ADR-4]]")),
        (Noise::Link, assistant("[note](docs/(old)decided)")),
        // Data.
        (
            Noise::Data,
            assistant("{\"decision\": \"going with plan b\", \"ok\": true}"),
        ),
        (
            Noise::Data,
            assistant("[{\"tool\": \"search\", \"note\": \"switching to fallback\"}]"),
        ),
        (
            Noise::Data,
            assistant("[ {\"note\": \"switching to fallback\"}]"),
        ),
        (Noise::Data, assistant("[\"decided\", \"pending\"]")),
        (
            Noise::Data,
            assistant("\"stack:\": [\"rust\", \"sqlite\"],"),
        ),
        (Noise::Data, assistant("{ \"решение:\": \"оставить\" }")),
        (Noise::Data, assistant("\"status\": \"decided\",")),
        (Noise::Data, assistant("'status': 'decided',")),
        (Noise::Data, assistant("\"\": \"decided\",")),
        (Noise::Data, assistant("\"a\\\"b\": \"decided\",")),
        (Noise::Data, assistant("\"switching to fallback\",")),
        (
            Noise::Data,
            user("{\"event\": \"stop\", \"reason\": \"limit\"}"),
        ),
        // A step of a numbered list.
        (
            Noise::ListStep,
            assistant("1. Сначала используем старый формат для проверки"),
        ),
        (
            Noise::ListStep,
            assistant("2. Затем переходим на новый формат конфигурации"),
        ),
        (
            Noise::ListStep,
            assistant("3) We are going with the staged rollout of it"),
        ),
        (
            Noise::ListStep,
            assistant("4. Switching to the release branch"),
        ),
        (
            Noise::ListStep,
            assistant("12. Если тесты прошли, выбираем следующий шаг"),
        ),
        (
            Noise::ListStep,
            user("Порядок такой:\n3. на этом шаге стоп, если тесты упали"),
        ),
        (
            Noise::ListStep,
            assistant("1.1. Затем переходим на новый формат конфигурации"),
        ),
        (
            Noise::ListStep,
            assistant("**1.** Сначала используем старый формат"),
        ),
        (
            Noise::ListStep,
            assistant("a) We are going with the staged rollout"),
        ),
        (
            Noise::ListStep,
            assistant("(1) Switching to the release branch"),
        ),
        (
            Noise::ListStep,
            assistant("1000) Switching to the release branch"),
        ),
        (
            Noise::NotADecision,
            assistant("Open .decided and inspect the log."),
        ),
        // A decision that asks.
        (Noise::Question, user("Что используем для очереди?")),
        (
            Noise::Question,
            assistant("Are we going with the second option?"),
        ),
        (Noise::Question, user("Какой вариант выбираем?")),
        (Noise::Question, assistant("Decided on the schema already?")),
        (Noise::Question, user("Переходим на новую версию или ждём?")),
        (
            Noise::Question,
            user("Что используем для очереди? Redis или NATS"),
        ),
        (Noise::Question, assistant("**Какой вариант выбираем?**")),
        (
            Noise::Question,
            assistant("(или переходим на новую версию?)"),
        ),
        // A phrase of a decision only inside a longer word or a name.
        (
            Noise::NotADecision,
            assistant("Используемые библиотеки перечислены в файле зависимостей."),
        ),
        (
            Noise::NotADecision,
            assistant("The team is still undecided about the cache."),
        ),
        (
            Noise::NotADecision,
            assistant("The callstack: main, run, poll."),
        ),
        (
            Noise::NotADecision,
            assistant("Ongoing with no changes since Monday."),
        ),
        (
            Noise::NotADecision,
            assistant("Неиспользуемый код удалён из модуля."),
        ),
        (Noise::NotADecision, assistant("if is_decided { return; }")),
        (
            Noise::NotADecision,
            assistant("The page is docs/blog/why-we-decided and it is long."),
        ),
        (
            Noise::NotADecision,
            assistant("The team is un\u{ad}decided about it."),
        ),
        (
            Noise::NotADecision,
            assistant("The run ended in the 3decided state."),
        ),
        (
            Noise::NotADecision,
            assistant("The token is decided\u{1ab0} and short."),
        ),
        // A marker of a correction only inside a longer word or a name.
        (
            Noise::NotACorrection,
            user("Смотри, мне надо сделать выгрузку отчёта за месяц."),
        ),
        (
            Noise::NotACorrection,
            user("Надо разобрать папку с отчётами и, может быть, не только её."),
        ),
        (
            Noise::NotACorrection,
            user("Проверь, работает ли интернет, и перезапусти загрузку."),
        ),
        (
            Noise::NotACorrection,
            user("Поверни график на девяносто градусов."),
        ),
        (
            Noise::NotACorrection,
            user("The build stopped after the second step."),
        ),
        (
            Noise::NotACorrection,
            user("The job was redone yesterday and looks fine."),
        ),
        (
            Noise::NotACorrection,
            user("Wrongly named columns are listed in the report."),
        ),
        (
            Noise::NotACorrection,
            user("Добавь стоп-слова в фильтр поиска."),
        ),
        (
            Noise::NotACorrection,
            user("Отсортируй exports/revert-candidates.csv по дате."),
        ),
        (
            Noise::NotACorrection,
            user("Open notes/stop_words.txt and sort the lines."),
        ),
        (
            Noise::NotACorrection,
            user("The runner printed stop2 and went on."),
        ),
        (
            Noise::NotACorrection,
            user("Open /stop and inspect the output."),
        ),
        (
            Noise::NotACorrection,
            user("Открой файл стоп.txt и отсортируй строки."),
        ),
        (
            Noise::NotACorrection,
            user("Смотри в папке заметки/стоп и не трогай остальное."),
        ),
        // A strong marker inside a phrase of everyday speech.
        (
            Noise::NotACorrection,
            user("Не так давно мы обновили зависимости, проверь сборку."),
        ),
        (
            Noise::NotACorrection,
            user("Не то чтобы срочно, но посмотри логи сборки."),
        ),
        (
            Noise::NotACorrection,
            user("Проверь, есть ли файл настроек; если нет, создай его."),
        ),
        // A reminder, whatever its apostrophe, and a negation inside a word.
        (
            Noise::NotACorrection,
            user("Please don\u{2019}t forget to tag the release after the merge."),
        ),
        (
            Noise::NotACorrection,
            user("We cannot forget the tag; the build stopped."),
        ),
    ];

    /// True statements the rules hide. They are here so that what a rule
    /// costs is written down and tested, not found later.
    pub const WRONGLY_HIDDEN: &[(Noise, Turn)] = &[
        (
            Noise::ListStep,
            assistant("Решения:\n1. Переходим на FastAPI\n2. Используем PostgreSQL"),
        ),
        (
            Noise::TableRow,
            assistant("| Decision: | use SQLite for the index |"),
        ),
        (
            Noise::Link,
            assistant("[Decision: use SQLite for the index](docs/adr-004.md)"),
        ),
        (
            Noise::Question,
            assistant("Which store are we going with?\nSQLite, it needs no server."),
        ),
        (
            Noise::Markup,
            assistant("<b>Decision: use SQLite for the index</b>"),
        ),
        (
            Noise::ImageLink,
            assistant("![Decision: use SQLite for the index](img/decision.png)"),
        ),
        (
            Noise::TableRow,
            assistant("We decided between a | b | c and took b."),
        ),
        (
            Noise::TableRow,
            assistant("| 1 | we are switching to the new parser |"),
        ),
        (
            Noise::Link,
            assistant("[We decided to keep it](notes/keep.md)"),
        ),
        (
            Noise::Link,
            assistant("[Решение: перейти на новый формат](docs/adr-007.md)"),
        ),
        (Noise::Link, assistant("[[Decision: use SQLite]]")),
        (
            Noise::ImageLink,
            assistant("![we decided on this layout](img/layout.png)"),
        ),
        (
            Noise::Markup,
            assistant("<details><summary>Decision: keep the old index</summary></details>"),
        ),
        (
            Noise::Markup,
            assistant("<вывод>решение: оставить как есть</вывод>"),
        ),
        (
            Noise::ListStep,
            assistant("3) We are going with the staged rollout"),
        ),
        (Noise::Question, user("Значит используем Redis?")),
    ];

    /// Junk that no rule names. It is here so that the limit of the rules is
    /// written down and a later rule has a test to turn.
    pub const PASSES: &[Turn] = &[
        assistant("echo \"decision: use SQLite\" >> notes.txt"),
        assistant("INFO worker: switching to fallback endpoint after 3 retries"),
        assistant("Открываем настройки и выбираем раздел отчётов."),
        user("stop the dev server and start it again"),
        assistant("We have not decided on the cache yet."),
        assistant("Not decided yet: cache backend."),
        assistant("Call stack: main, run, poll."),
        assistant("Architecture: arm64"),
        assistant("Step 3: switching to the release branch"),
        assistant("- [ ] Switching to the release branch"),
        assistant("How is it going with the migration, any blockers left"),
        user("Да нет, всё нормально, продолжай."),
        user("Что используем для очереди, Redis или NATS?"),
        assistant("Are we going with the second option, or the first?"),
        assistant("## Что используем"),
        assistant("`<result>` decided"),
        assistant("```decision: nothing\nthe body of the block\n```"),
        assistant("```yaml\narchitecture: arm64\n```"),
        assistant("+    let decided = true;"),
        assistant("    if decided { return; }"),
    ];

    /// Stored contents of shapes capture had earlier, judged as they are
    /// stored. Until September 2026 a decision was stored as the first line
    /// of its reply wherever the phrase stood, and until June 2026 a
    /// correction was taken by words that are markers no more.
    pub const STORED_EARLIER: &[(Capture, &str, Option<Noise>)] = &[
        (Capture::Decision, "Done.", Some(Noise::NotADecision)),
        (Capture::Decision, "Decision: keep the smaller index.", None),
        (
            Capture::Decision,
            "| Настройка | Что выбираем |",
            Some(Noise::TableRow),
        ),
        (
            Capture::Correction,
            "Лучше считать хеш по содержимому файла.",
            Some(Noise::NotACorrection),
        ),
        (
            Capture::Correction,
            "It would be better to hash the content of the file.",
            Some(Noise::NotACorrection),
        ),
        (
            Capture::Correction,
            "Это неправильно, лучше считать хеш по содержимому.",
            None,
        ),
    ];

    /// Replies in which the line that states and the headline are two
    /// lines (a fence, a row or a line without a marker comes first), or
    /// in which no line states: the reply, the statement line of what is
    /// stored, its headline, and the verdict.
    pub const LINES: &[(Turn, Option<&str>, &str, Option<Noise>)] = &[
        // The reply opens with a line too short for a headline, and the
        // headline skips it. The decision is stored from the line that
        // matched, so there the two are one line.
        (
            assistant("ok\nDecision: keep the larger index.\nIt is enough for the tests."),
            Some("Decision: keep the larger index."),
            "Decision: keep the larger index.",
            None,
        ),
        (
            user("ok\nAbout the cache.\nstop caching errors."),
            Some("stop caching errors."),
            "About the cache.",
            None,
        ),
        // A correction keeps the whole reply, and its headline is its
        // first line. The only line with a marker is a row.
        (
            user("About the import of yesterday.\n| stop doing that | now |"),
            None,
            "About the import of yesterday.",
            Some(Noise::TableRow),
        ),
        (
            user("About the export of yesterday.\nstop, use the other file"),
            Some("stop, use the other file"),
            "About the export of yesterday.",
            None,
        ),
        // A fence is no headline, and it is the statement line of what
        // capture stored from it.
        (
            assistant("```decision: none yet\nthe body of the block\n```"),
            Some("```decision: none yet"),
            "the body of the block",
            None,
        ),
    ];

    /// What stands inside the private block of the replies below.
    pub const PRIVATE_TEXT: &str = "a name";

    /// Replies with a private block, which redaction replaces by its marker
    /// before anything is selected: the reply, whether the marker is in
    /// what is stored, and the verdict.
    pub const REDACTED: &[(Turn, bool, Option<Noise>)] = &[
        (
            assistant("<private>a name</private>\nDecision: use SQLite for the cache"),
            false,
            None,
        ),
        (
            assistant("<private>a name</private> | decided | yes |"),
            true,
            Some(Noise::TableRow),
        ),
        (
            assistant("- <private>a name</private> 1. Decision: keep the old index"),
            true,
            None,
        ),
        (
            assistant("<private>a name</private> - {\"decided\": true}"),
            true,
            Some(Noise::Data),
        ),
        (
            assistant("<private>a name</private> {\"decided\": true}"),
            true,
            Some(Noise::Data),
        ),
        (
            assistant("1. <private>a name</private> are going with the rollout"),
            true,
            Some(Noise::ListStep),
        ),
        (
            assistant("<private>a name</private> decided?"),
            true,
            Some(Noise::Question),
        ),
        (
            assistant("<private>a name</private> decided to keep it"),
            true,
            None,
        ),
    ];

    /// Texts with references to attachments, closed and not.
    pub const ATTACHMENTS: &[&str] = &[
        "@\"/srv/app/users.csv\" import these rows into the test base",
        "compare with @\"/opt/data/partial",
        "a reply without references",
        "@\"one.txt\" and @\"two.txt\" stop here",
        "@\"\" stop here",
        "an address like dev@\"example\" stop here",
    ];

    /// Corrections that only the strict predicate takes: their marker is a
    /// word capture does not reach.
    pub const STRICT_CORRECTIONS: &[&str] = &[
        "Это не та ветка, нужна release.",
        "Это не те файлы, смотри в папке docs.",
    ];

    /// Replies the tests of detection hold as no correction. Given to the
    /// rules as if capture had taken them for one, each is noise.
    pub const NO_CORRECTION: &[&str] = &[
        "Добавь JWT авторизацию",
        "покажи мне код",
        "Какой формат лучше подойдёт для экспорта отчётов: CSV или Parquet?",
        "Would it be better to cache the parsed config between runs?",
        "Не надо ли увеличить таймаут? Или тесты падают по другой причине? Лог прикладываю ниже.",
        "Перед релизом не забудь поднять версию в Cargo.toml.",
        "Please don't forget to tag the release after the merge.",
        "Подскажи, какую библиотеку лучше взять для разбора дат в этом модуле.",
        "@\"/home/dev/exports/revert-candidates.csv\" Отсортируй строки по дате.",
    ];

    /// Replies the tests of detection hold as no decision, with the rule
    /// that names each when it is given as one. None holds a phrase.
    pub const NO_DECISION: &[&str] = &[
        "покажи мне код",
        "что думаешь?",
        "Here is the code: fn foo() { println!(\"hi\"); }",
    ];

    /// One reply for each step of the predicate of corrections, with what
    /// the step makes of it. Capture gives the same for each.
    pub const CORRECTION_STEPS: &[(&str, &str, bool)] = &[
        ("an empty reply", "   ", false),
        (
            "a reference to an attachment alone",
            "@\"notes/stop-words.txt\"",
            false,
        ),
        ("a question at the end", "stop the import?", false),
        (
            "the end is looked at without the spaces behind it",
            "stop the import?  ",
            false,
        ),
        ("a strong marker", "please stop the import", true),
        (
            "a strong marker goes before a negation",
            "stop, and don't forget the tag",
            true,
        ),
        (
            "a strong marker goes before two question marks",
            "stop? why? see below, it is broken",
            true,
        ),
        ("a negation", "don't forget the forgotten tag", false),
        (
            "a marker that negation can turn",
            "forget the second option",
            true,
        ),
        (
            "a marker that negation can turn goes before two question marks",
            "forget that? no? fine then",
            true,
        ),
        ("two question marks", "wrong? or right? see below", false),
        (
            "one question mark does not refuse",
            "wrong column? use the other one",
            true,
        ),
        (
            "a weak marker near the start",
            "wrong column, use the other",
            true,
        ),
        (
            "a weak marker far from the start",
            "the report lists every column of the table and one is wrong",
            false,
        ),
    ];

    /// Texts for the window of a weak marker: the text, the marker, and
    /// whether the marker stands as a word within the first 50 characters.
    pub fn window_cases() -> Vec<(String, &'static str, bool)> {
        vec![
            (format!("{}wrongly", "x ".repeat(22)), "wrong", false),
            (format!("{}wrong way", "x ".repeat(22)), "wrong", true),
            // A whole word that ends where the window ends, and one that
            // ends a character behind it.
            (format!("{} wrong way", "x ".repeat(22)), "wrong", true),
            (format!("{}wrong way", "x ".repeat(23)), "wrong", false),
            // The marker starts inside the window and ends outside.
            (format!("{}wrong", "x ".repeat(24)), "wrong", false),
            // The window ends where the marker ends, and the word goes on.
            (format!("{} wrongly", "x ".repeat(22)), "wrong", false),
            // The same in a script of two bytes a letter.
            (format!("{} не надоело", "я ".repeat(21)), "не надо", false),
            (format!("{} не надо так", "я ".repeat(21)), "не надо", true),
        ]
    }

    /// Replies at the limit of length, which is counted in bytes: the text
    /// and whether it is a correction.
    pub fn limit_cases() -> Vec<(String, bool)> {
        vec![
            (format!("stop {}", "x".repeat(495)), true),
            (format!("stop {}", "x".repeat(496)), false),
            // 255 characters, 509 bytes.
            (format!("стоп {}", "я".repeat(250)), false),
            // 250 characters, 499 bytes.
            (format!("стоп {}", "я".repeat(245)), true),
            // The apostrophe of a typesetter is three bytes as written.
            (format!("that\u{2019}s wrong, {}", "x".repeat(486)), false),
            (format!("that\u{2019}s wrong, {}", "x".repeat(484)), true),
        ]
    }

    /// Long replies of the kind the tests of detection build, which
    /// capture stores as decisions cut at 1000 characters.
    pub fn long_kept() -> Vec<(&'static str, String)> {
        vec![
            ("user", "давай используем ".repeat(70)),
            (
                "assistant",
                format!("Done.\nDecision: {}", "решение 🚀 ".repeat(200)),
            ),
            (
                "assistant",
                format!(
                    "SENTINEL\nDecision: use SQLite for demoapp.\n{}\nthe far end",
                    "x".repeat(1100)
                ),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_data_the_marker_opening_is_how_redaction_writes_its_markers() {
        for marker in [
            crate::redaction::CREDENTIAL_MARKER,
            crate::redaction::PRIVATE_MARKER,
        ] {
            assert!(marker.starts_with(REDACTION_MARKER_OPEN));
            assert!(marker.ends_with(']'));
        }
    }

    #[test]
    fn quality_data_lists_are_in_lower_case_and_without_doubles() {
        for list in [
            CORRECTION_STRONG,
            CORRECTION_IDIOMS,
            CORRECTION_NEGATABLE,
            CORRECTION_NEGATED,
            CORRECTION_WEAK,
            DECISION,
        ] {
            let mut seen = std::collections::HashSet::new();
            for word in list {
                assert_eq!(*word, word.to_lowercase(), "{word}");
                assert_eq!(*word, word.trim(), "{word}");
                assert!(seen.insert(*word), "twice: {word}");
            }
        }
    }

    #[test]
    fn quality_data_every_idiom_holds_a_strong_marker() {
        for idiom in CORRECTION_IDIOMS {
            assert!(
                CORRECTION_STRONG
                    .iter()
                    .any(|marker| idiom.contains(marker)),
                "{idiom}"
            );
        }
    }

    #[test]
    fn quality_data_there_are_five_hidden_turns_for_every_rule() {
        for rule in crate::quality::Noise::ALL {
            let count = fixtures::HIDDEN.iter().filter(|(r, _)| *r == rule).count();
            assert!(count >= 5, "{}: {count}", rule.as_str());
        }
    }

    #[test]
    fn quality_data_no_text_is_a_fixture_twice() {
        let mut seen = std::collections::HashSet::new();
        let texts = fixtures::KEPT
            .iter()
            .chain(fixtures::PASSES)
            .chain(fixtures::HIDDEN.iter().map(|(_, turn)| turn))
            .chain(fixtures::WRONGLY_HIDDEN.iter().map(|(_, turn)| turn));
        for turn in texts {
            assert!(seen.insert(turn.text), "twice: {}", turn.text);
        }
    }

    #[test]
    fn quality_data_both_scripts_are_among_the_turns_of_wording() {
        use crate::quality::Noise;
        for rule in [Noise::NotACorrection, Noise::NotADecision] {
            let texts: Vec<_> = fixtures::HIDDEN
                .iter()
                .filter(|(r, _)| *r == rule)
                .map(|(_, turn)| turn.text)
                .collect();
            assert!(
                texts.iter().any(|t| fixtures::is_cyrillic(t)),
                "{}",
                rule.as_str()
            );
            assert!(
                texts.iter().any(|t| !fixtures::is_cyrillic(t)),
                "{}",
                rule.as_str()
            );
        }
    }
}
