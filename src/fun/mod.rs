//! What makes the office feel lived in, decided apart from how it is drawn:
//! the office's own clock (lunch, overtime, the festivals), how a stuck agent
//! takes it, what the chitchat says, the day's tally and Hong Kong's weather.

pub mod hko;
pub mod stats;

use std::time::{Duration, SystemTime};

use pixtuoid_core::state::ActivityState;
use pixtuoid_core::{AgentId, AgentSlot};

use crate::tasks::{Cheer, Phase, Stuck, TaskView};

/// Hong Kong is UTC+8 all year.
const HONG_KONG_UTC_OFFSET: i32 = 8 * 3600;

/// The zone the office keeps its time in, as seconds east of UTC:
/// `AGENT_OFFICE_UTC_OFFSET`, else Hong Kong's, whatever zone the machine is in.
pub fn utc_offset() -> i32 {
    std::env::var("AGENT_OFFICE_UTC_OFFSET")
        .ok()
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|o| o.abs() < 24 * 3600)
        .unwrap_or(HONG_KONG_UTC_OFFSET)
}

/// A moment on the office's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    /// 0 is Monday.
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
}

impl Clock {
    /// `now` in the zone `secs_east` of UTC.
    pub fn at(now: SystemTime, secs_east: i32) -> Self {
        let unix = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            + i64::from(secs_east);
        let days = unix.div_euclid(86_400);
        let secs = unix.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month,
            day,
            // 1970-01-01 was a Thursday.
            weekday: (days + 3).rem_euclid(7) as u32,
            hour: (secs / 3600) as u32,
            minute: (secs % 3600 / 60) as u32,
        }
    }

    /// `now` on the office's clock.
    pub fn now(now: SystemTime) -> Self {
        Self::at(now, utc_offset())
    }

    /// The date as `YYYY-MM-DD`, a day's key.
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    fn minutes(&self) -> u32 {
        self.hour * 60 + self.minute
    }

    /// Lunch hour: a weekday's 12:30 to 13:30, when the idle go down to the
    /// cha chaan teng.
    pub fn lunch(&self) -> bool {
        self.weekday < 5 && (12 * 60 + 30..13 * 60 + 30).contains(&self.minutes())
    }

    /// Late: from 22:00 to 06:00, when only those still working keep a light on.
    pub fn late(&self) -> bool {
        self.hour >= 22 || self.hour < 6
    }

    /// The evening, when the wall board turns to the day's tally.
    pub fn evening(&self) -> bool {
        self.hour >= 18
    }

    /// The festival on now, if any.
    pub fn festival(&self) -> Option<Festival> {
        let today = days_from_civil(self.year, self.month, self.day);
        let near = |table: &[(i64, u32, u32)], before: i64, after: i64| {
            table.iter().any(|&(y, m, d)| {
                let day = days_from_civil(y, m, d);
                (day - before..=day + after).contains(&today)
            })
        };
        // New Year's eve until the Lantern Festival, the fifteenth day.
        if near(LUNAR_NEW_YEAR, 1, 14) {
            return Some(Festival::LunarNewYear);
        }
        if near(MID_AUTUMN, 2, 1) {
            return Some(Festival::MidAutumn);
        }
        if self.month == 12 && (18..=26).contains(&self.day) {
            return Some(Festival::Christmas);
        }
        None
    }
}

/// Days since 1970-01-01 to a (year, month, day) (Hinnant's civil_from_days).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// A (year, month, day) to days since 1970-01-01 (Hinnant's days_from_civil).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The first day of each lunar year, on the Gregorian calendar.
const LUNAR_NEW_YEAR: &[(i64, u32, u32)] = &[
    (2025, 1, 29),
    (2026, 2, 17),
    (2027, 2, 6),
    (2028, 1, 26),
    (2029, 2, 13),
    (2030, 2, 3),
    (2031, 1, 23),
    (2032, 2, 11),
    (2033, 1, 31),
    (2034, 2, 19),
    (2035, 2, 8),
];

/// The fifteenth day of each eighth lunar month.
const MID_AUTUMN: &[(i64, u32, u32)] = &[
    (2025, 10, 6),
    (2026, 9, 25),
    (2027, 9, 15),
    (2028, 10, 3),
    (2029, 9, 22),
    (2030, 9, 12),
    (2031, 10, 1),
    (2032, 9, 19),
    (2033, 9, 8),
    (2034, 9, 27),
    (2035, 9, 16),
];

/// A festival the office dresses up for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Festival {
    LunarNewYear,
    MidAutumn,
    Christmas,
}

/// How long a cheer plays after what earned it.
pub const CHEER_FOR: Duration = Duration::from_secs(6);

/// The cheer `view` has earned that still plays at `now`.
pub fn live_cheer(view: Option<&TaskView>, now: SystemTime) -> Option<(Cheer, Duration)> {
    let (cheer, at) = view?.cheer?;
    let ago = now.duration_since(at).ok()?;
    (ago < CHEER_FOR).then_some((cheer, ago))
}

/// A working turn this long is overtime whatever the hour.
pub const OVERTIME_TURN: Duration = Duration::from_secs(30 * 60);

/// Whether `slot` is working overtime at `now`: still at it late at night, or
/// on a turn that has run half an hour.
pub fn overtime(slot: &AgentSlot, view: Option<&TaskView>, clock: &Clock, now: SystemTime) -> bool {
    if slot.exiting_at.is_some() || !matches!(slot.state, ActivityState::Running { .. }) {
        return false;
    }
    let long = view
        .and_then(|v| v.turn_started)
        .and_then(|t| now.duration_since(t).ok())
        .is_some_and(|d| d >= OVERTIME_TURN);
    long || clock.late()
}

/// How a stuck agent takes it, worse the longer it lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mood {
    /// Failing over and over: scratching its head, sweating.
    Frustrated,
    /// Gone quiet: slumped over the desk.
    Slumped,
    /// Long stuck: a rain cloud of its own.
    Gloomy,
}

/// Failures in a row that bring the cloud.
const GLOOMY_FAILS: u32 = 6;
/// Quiet minutes that bring the cloud.
const GLOOMY_QUIET_MIN: u64 = 15;

impl Mood {
    pub fn of(stuck: Stuck) -> Self {
        match stuck {
            Stuck::Failing(n) if n >= GLOOMY_FAILS => Mood::Gloomy,
            Stuck::Failing(_) => Mood::Frustrated,
            Stuck::Quiet(m) if m >= GLOOMY_QUIET_MIN => Mood::Gloomy,
            Stuck::Quiet(_) => Mood::Slumped,
        }
    }
}

/// Which of the stuck the shop cat goes to sit with: the worst off, then the
/// longest stuck, so it settles on one and stays.
pub fn cat_visits(stuck: &[(AgentId, Stuck)]) -> Option<AgentId> {
    stuck
        .iter()
        .max_by_key(|(id, s)| {
            let weight = match s {
                Stuck::Failing(n) => u64::from(*n),
                Stuck::Quiet(m) => *m,
            };
            (Mood::of(*s), weight, std::cmp::Reverse(*id))
        })
        .map(|(id, _)| *id)
}

/// What chitchat is about right now, office-wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occasion {
    Ordinary,
    Lunch,
    Typhoon,
}

const OFFICE_TALK: &[&str] = &[
    "饮咗茶未？",
    "埋单先！",
    "又改需求…",
    "今晚 OT 到几点？",
    "今晚食咩好？",
    "老细又嚟喇",
    "赶 deadline 呀",
    "开会开到傻",
    "周末去行山？",
    "落雨收衫喇",
    "好攰呀…",
    "加油！",
    "Ship 咗未？",
    "下午茶叫外卖？",
];

const LUNCH_ORDERS: &[&str] = &[
    "A 餐，冻奶茶",
    "菠萝油加牛油",
    "常餐走葱",
    "热柠茶少甜",
    "叉烧饭加蛋",
    "餐蛋面！",
    "冻柠茶走甜",
    "西多士一份",
];

const TYPHOON_TALK: &[&str] = &[
    "八号风球喇！",
    "收工返屋企",
    "快啲走，冇车喇",
    "天星停航喇",
    "记得关窗",
];

/// What an agent at `phase` has to say about its work.
fn shop_talk(phase: Phase) -> &'static [&'static str] {
    match phase {
        Phase::Research => &["睇紧文档", "搵到喇！", "呢个 API 好怪"],
        Phase::Code => &["快搞掂", "再改少少", "边个写嘅？"],
        Phase::Test => &["又红咗…", "test 过晒未？", "等 CI 中"],
        Phase::Run => &["跑紧…", "build 好慢", "等等先"],
        Phase::Delegate => &["交俾细佬做", "开会讲下", "分工合作"],
    }
}

/// What `speaker` says in turn `turn` of a chitchat: lunch orders at lunch,
/// the typhoon in a typhoon, else its own work half the time and office talk
/// the rest. A cheer still playing is what it talks about.
pub fn chitchat(
    speaker: AgentId,
    view: Option<&TaskView>,
    occasion: Occasion,
    turn: u64,
    now: SystemTime,
) -> &'static str {
    let seed = mix(speaker.raw() ^ turn.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    let pick = |lines: &'static [&'static str]| lines[(seed % lines.len() as u64) as usize];
    match occasion {
        Occasion::Typhoon => return pick(TYPHOON_TALK),
        Occasion::Lunch => return pick(LUNCH_ORDERS),
        Occasion::Ordinary => {}
    }
    if let Some((cheer, _)) = live_cheer(view, now) {
        return match cheer {
            Cheer::TestsPassed => "全部绿晒！",
            Cheer::Shipped => "推咗上去喇",
            Cheer::Recovered => "终于搞掂！",
        };
    }
    match view.and_then(|v| v.phase) {
        Some(p) if seed >> 32 & 1 == 0 => pick(shop_talk(p)),
        _ => pick(OFFICE_TALK),
    }
}

/// A cheap, well-spread hash (splitmix64's finaliser).
pub fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i64, mo: u32, d: u32, h: u32, mi: u32) -> SystemTime {
        let secs = days_from_civil(y, mo, d) * 86_400 + i64::from(h * 3600 + mi * 60);
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs as u64)
    }

    /// The office keeps Hong Kong's clock whatever zone the machine is in:
    /// 20:00 UTC is already the next morning there, so lunch, the late shift
    /// and the festivals must turn on that day, not the machine's.
    #[test]
    fn the_clock_reads_hong_kong_time_and_date() {
        let c = Clock::at(at(2026, 12, 31, 20, 15), HONG_KONG_UTC_OFFSET);
        assert_eq!(
            (c.year, c.month, c.day, c.hour, c.minute),
            (2027, 1, 1, 4, 15)
        );
        assert_eq!(c.weekday, 4, "1 Jan 2027 is a Friday");
        assert_eq!(c.date(), "2027-01-01");
        for z in [-800_000, 0, 20_000, 60_000] {
            assert_eq!(days_from_civil_round_trip(z), z);
        }
    }

    fn days_from_civil_round_trip(z: i64) -> i64 {
        let (y, m, d) = civil_from_days(z);
        days_from_civil(y, m, d)
    }

    /// Lunch is a weekday's 12:30 to 13:30 only; on a Saturday no one is
    /// marched down to the cha chaan teng.
    #[test]
    fn lunch_is_a_weekday_half_past_twelve_for_an_hour() {
        let lunch = |d, h, m| Clock::at(at(2026, 11, d, h, m), 0).lunch();
        assert!(lunch(5, 12, 30), "Thursday 12:30");
        assert!(lunch(5, 13, 29));
        assert!(!lunch(5, 13, 30));
        assert!(!lunch(5, 12, 29));
        assert!(!lunch(7, 12, 45), "Saturday");
    }

    /// Each festival dresses the office for its own days: the lunar ones move
    /// on the Gregorian calendar, so they come from the table, and a day out
    /// of every window is an ordinary one.
    #[test]
    fn festivals_fall_on_their_days() {
        let fest = |y, m, d| Clock::at(at(y, m, d, 10, 0), 0).festival();
        assert_eq!(fest(2027, 2, 6), Some(Festival::LunarNewYear));
        assert_eq!(
            fest(2027, 2, 20),
            Some(Festival::LunarNewYear),
            "to the 15th day"
        );
        assert_eq!(fest(2027, 2, 21), None);
        assert_eq!(fest(2027, 9, 15), Some(Festival::MidAutumn));
        assert_eq!(fest(2026, 12, 25), Some(Festival::Christmas));
        assert_eq!(fest(2026, 11, 5), None);
    }

    /// A stuck agent takes it worse the longer it lasts, and the cat sits
    /// with the worst off, so the eye goes there first.
    #[test]
    fn the_longer_stuck_the_gloomier_and_the_cat_goes_to_the_worst() {
        assert_eq!(Mood::of(Stuck::Failing(3)), Mood::Frustrated);
        assert_eq!(Mood::of(Stuck::Failing(6)), Mood::Gloomy);
        assert_eq!(Mood::of(Stuck::Quiet(5)), Mood::Slumped);
        assert_eq!(Mood::of(Stuck::Quiet(20)), Mood::Gloomy);
        let a = AgentId::from_parts("claude-code", "a");
        let b = AgentId::from_parts("claude-code", "b");
        let c = AgentId::from_parts("claude-code", "c");
        let stuck = [
            (a, Stuck::Failing(4)),
            (b, Stuck::Quiet(30)),
            (c, Stuck::Quiet(16)),
        ];
        assert_eq!(cat_visits(&stuck), Some(b));
        assert_eq!(cat_visits(&[]), None);
    }

    /// The chitchat fits the hour: orders at lunch, the signal in a typhoon,
    /// a cheer when one is playing; and each turn may say something new.
    #[test]
    fn chitchat_fits_the_occasion() {
        let id = AgentId::from_parts("claude-code", "a");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        for turn in 0..20 {
            assert!(LUNCH_ORDERS.contains(&chitchat(id, None, Occasion::Lunch, turn, now)));
            assert!(TYPHOON_TALK.contains(&chitchat(id, None, Occasion::Typhoon, turn, now)));
        }
        let cheered = TaskView {
            cheer: Some((Cheer::TestsPassed, now - Duration::from_secs(1))),
            ..TaskView::default()
        };
        assert_eq!(
            chitchat(id, Some(&cheered), Occasion::Ordinary, 3, now),
            "全部绿晒！"
        );
        let said: std::collections::HashSet<_> = (0..40)
            .map(|t| chitchat(id, None, Occasion::Ordinary, t, now))
            .collect();
        assert!(said.len() > 5, "the talk varies turn to turn");
    }

    /// Overtime is a working agent late at night, or one on a half-hour turn;
    /// an idle one is never on overtime, however late.
    #[test]
    fn overtime_is_working_late_or_long() {
        use pixtuoid_core::source::{AgentEvent, Transport};
        use pixtuoid_core::{Reducer, SceneState};
        let id = AgentId::from_parts("claude-code", "s");
        let mut scene = SceneState::uniform(4);
        let now = at(2026, 11, 5, 3, 0);
        Reducer::new().apply(
            &mut scene,
            AgentEvent::SessionStart {
                agent_id: id,
                source: "claude-code".into(),
                session_id: "s".into(),
                cwd: "/repo".into(),
                parent_id: None,
            },
            now,
            Transport::Hook,
        );
        let slot = scene.agents.get_mut(&id).unwrap();
        let (day, night) = (Clock::at(at(2026, 11, 5, 15, 0), 0), Clock::at(now, 0));
        slot.state = ActivityState::Running { item: None };
        assert!(overtime(slot, None, &night, now));
        assert!(!overtime(slot, None, &day, now));
        let long = TaskView {
            turn_started: Some(now - OVERTIME_TURN),
            ..TaskView::default()
        };
        assert!(overtime(slot, Some(&long), &day, now));
        slot.state = ActivityState::Idle;
        assert!(!overtime(slot, Some(&long), &night, now));
    }
}
