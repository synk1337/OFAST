//! A demo's overview, for choosing what to film: the teams, each round's winner, score and kill feed, and the moments
//! a fragmovie is made of (multi-kills, clutches, collaterals, no-scopes, wallbangs…), read in one pass.
//!
//! Round winners and scores come from entities, not events: tournament demos (SourceTV) carry no `round_end` event.
//! The game rules entity counts the round ends (`m_nRoundEndCount`) and keeps the last one's winner
//! (`m_iRoundEndWinnerTeam`) and reason (`m_eRoundEndReason`); each side's team entity (`CCSTeam`) has the clan's
//! name and score. A demo that has `round_end` events (older matchmaking demos) is read from them as well.
//!
//! The highlights are worked out from the rounds alone ([`highlights`]), so they can be tested without a demo.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::decoder::FieldValue;
use super::entity::Entity;
use super::events::GameEvent;
use super::fieldpath::FieldPath;
use super::parser::DemoParser;
use super::players::{COUNTER_TERRORISTS, Paths, RoundEvent, TERRORISTS, TrackError, walk_rounds};

/// Everything [`summarize`] read from a demo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DemoSummary {
    /// Map name from the demo header.
    pub map: String,
    /// The server's name from the header (a tournament's name on its demos).
    pub server: String,
    /// Seconds per tick.
    pub tick_interval: f32,
    /// Everyone who played a round, in order of first appearance. [`Kill`]s and [`Highlight`]s point into it.
    pub players: Vec<Player>,
    /// The rounds after warmup, in order.
    pub rounds: Vec<RoundSummary>,
    /// The demo's highlights, in round and tick order ([`highlights`]).
    pub highlights: Vec<Highlight>,
}

/// One player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    /// The name shown in the game.
    pub name: String,
    /// The 64-bit Steam id as text (a JavaScript number cannot hold it exactly), `"0"` for a bot.
    pub steam_id: String,
}

/// A side of the match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// The terrorists (team 2).
    T,
    /// The counter-terrorists (team 3).
    Ct,
}

impl Side {
    /// The side of a team number, `None` for spectators and unassigned players.
    pub fn of_team(team: u32) -> Option<Self> {
        match team {
            TERRORISTS => Some(Self::T),
            COUNTER_TERRORISTS => Some(Self::Ct),
            _ => None,
        }
    }

    /// The other side.
    pub fn other(self) -> Self {
        match self {
            Self::T => Self::Ct,
            Self::Ct => Self::T,
        }
    }
}

/// Why a round ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundEnd {
    /// The bomb exploded.
    BombExploded,
    /// The bomb was defused.
    BombDefused,
    /// The losing side was eliminated.
    Eliminated,
    /// The time ran out without a plant.
    TimeRanOut,
    /// A side surrendered.
    Surrender,
    /// A draw.
    Draw,
    /// Another reason (hostage maps, …).
    Other,
}

impl RoundEnd {
    /// The reason of a `round_end` event's `reason` key or the rules entity's `m_eRoundEndReason` (Valve's
    /// `RoundEndReason` numbered from 1, as demoparser2 reads them: 1 target bombed, 7 bomb defused, 8 CTs win by
    /// elimination, 9 Ts win by elimination, 10 draw, 12 target saved, 17 / 18 surrender).
    pub fn of_reason(reason: i64) -> Self {
        match reason {
            1 => Self::BombExploded,
            7 => Self::BombDefused,
            8 | 9 => Self::Eliminated,
            10 => Self::Draw,
            12 => Self::TimeRanOut,
            17 | 18 => Self::Surrender,
            _ => Self::Other,
        }
    }
}

/// One side's team as a round ended.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Team {
    /// The clan's name (a tournament team's name), empty when the server sets none.
    pub name: String,
    /// The team's score once the round was over.
    pub score: i32,
}

/// One player's place in a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundPlayer {
    /// Index into [`DemoSummary::players`].
    pub player: usize,
    /// The side they played.
    pub side: Side,
}

/// One round.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundSummary {
    /// Round number (1 = first round after warmup).
    pub round: u32,
    /// First tick (freeze time starts).
    pub start_tick: u32,
    /// When the freeze time ended (`round_freeze_end`), if the demo has it.
    pub freeze_end_tick: Option<u32>,
    /// When the round was decided (the winner known; the kills after it are the seconds before the next round).
    pub decided_tick: Option<u32>,
    /// Last tick that belongs to the round.
    pub end_tick: u32,
    /// The side that won, `None` when the demo stops before the round is decided.
    pub winner: Option<Side>,
    /// Why it ended.
    pub reason: Option<RoundEnd>,
    /// The round was played again (a restart, a backup restored): a later round has its number, and this one's
    /// result did not count.
    pub restarted: bool,
    /// The terrorists' team as the round ended.
    pub t: Team,
    /// The counter-terrorists' team as the round ended.
    pub ct: Team,
    /// The players of both sides when the freeze time ended.
    pub players: Vec<RoundPlayer>,
    /// The round's kills, oldest first.
    pub kills: Vec<Kill>,
}

impl RoundSummary {
    /// Records how the round ended, unless it was decided already; `false` when it was, or when this end decides
    /// nothing (no winner and no draw: a restart during the seconds after the round counts a round end too).
    pub fn decide(&mut self, winner: Option<Side>, reason: RoundEnd, tick: u32) -> bool {
        if self.decided_tick.is_some() || (winner.is_none() && reason != RoundEnd::Draw) {
            return false;
        }
        self.winner = winner;
        self.reason = Some(reason);
        self.decided_tick = Some(tick);
        true
    }
}

/// What kind of weapon a kill was made with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeaponClass {
    /// AWP, SSG 08 and the auto-snipers.
    Sniper,
    /// Assault rifles.
    Rifle,
    /// Submachine guns.
    Smg,
    /// Shotguns and machine guns.
    Heavy,
    /// Pistols.
    Pistol,
    /// Knives.
    Knife,
    /// Grenades and fire.
    Grenade,
    /// The Zeus.
    Taser,
    /// The world, the bomb, …
    Other,
}

impl WeaponClass {
    /// The class of a `player_death` event's `weapon` name (`ak47`, `knife_t`, `inferno`, …).
    pub fn of(weapon: &str) -> Self {
        match weapon {
            "awp" | "ssg08" | "g3sg1" | "scar20" => Self::Sniper,
            "ak47" | "m4a1" | "m4a1_silencer" | "m4a1_silencer_off" | "famas" | "galilar"
            | "aug" | "sg556" => Self::Rifle,
            "mac10" | "mp9" | "mp7" | "mp5sd" | "ump45" | "p90" | "bizon" => Self::Smg,
            "nova" | "xm1014" | "sawedoff" | "mag7" | "m249" | "negev" => Self::Heavy,
            "glock" | "hkp2000" | "usp_silencer" | "usp_silencer_off" | "p250" | "fiveseven"
            | "tec9" | "cz75a" | "deagle" | "elite" | "revolver" => Self::Pistol,
            "hegrenade" | "inferno" | "molotov" | "incgrenade" | "flashbang" | "smokegrenade"
            | "decoy" => Self::Grenade,
            "taser" => Self::Taser,
            w if w.starts_with("knife") || w == "bayonet" => Self::Knife,
            _ => Self::Other,
        }
    }
}

/// One kill, from a `player_death` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Kill {
    /// The event's tick.
    pub tick: u32,
    /// Who died (index into [`DemoSummary::players`]).
    pub victim: Option<usize>,
    /// Who killed them, `None` for the world (a fall, the bomb).
    pub attacker: Option<usize>,
    /// Who helped.
    pub assister: Option<usize>,
    /// The weapon's name as the kill feed has it (`ak47`, `awp`, `knife_karambit`, `inferno`…).
    pub weapon: String,
    /// What kind of weapon it is.
    pub weapon_class: WeaponClass,
    /// A headshot.
    pub headshot: bool,
    /// Through a wall or another player (`penetrated`).
    pub wallbang: bool,
    /// With a sniper rifle, unscoped.
    pub noscope: bool,
    /// Through a smoke.
    pub through_smoke: bool,
    /// The killer was flashed.
    pub blind: bool,
    /// The killer was in the air.
    pub airborne: bool,
    /// The assister's flash blinded the victim.
    pub flash_assist: bool,
    /// The killer was on the victim's side (a team kill; a suicide is not one).
    pub team_kill: bool,
}

impl Kill {
    /// A kill that counts for the killer: of an opponent, by a player.
    pub fn of_opponent(&self) -> bool {
        self.attacker.is_some()
            && self.victim.is_some()
            && self.attacker != self.victim
            && !self.team_kill
    }
}

/// What makes a [`Highlight`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HighlightKind {
    /// Three or more kills in a round by one player ([`Highlight::count`] kills; 5 is an ace).
    MultiKill,
    /// The last player of a side won the round against [`Highlight::count`] opponents.
    Clutch,
    /// [`Highlight::count`] kills with one bullet.
    Collateral,
    /// A kill out of the ordinary ([`Highlight::tags`] says how).
    SpecialKill,
}

/// How a [`HighlightKind::SpecialKill`] stands out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillTag {
    /// A sniper kill without the scope.
    Noscope,
    /// Through a wall.
    Wallbang,
    /// Through a smoke.
    ThroughSmoke,
    /// In the air.
    Airborne,
    /// While flashed.
    Blind,
    /// With a knife.
    Knife,
}

/// A moment worth filming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Highlight {
    /// What it is.
    pub kind: HighlightKind,
    /// The round it happened in: its index in [`DemoSummary::rounds`] (not its number, which a restarted round
    /// shares with the round played again).
    pub round_index: usize,
    /// Who made it (index into [`DemoSummary::players`]).
    pub player: usize,
    /// Kills for a multi-kill or a collateral, opponents for a clutch (the X of "1vX"), 1 for a special kill.
    pub count: u32,
    /// How a special kill stands out (empty for the other kinds).
    pub tags: Vec<KillTag>,
    /// Where it starts: its first kill, or for a clutch the kill that left the player alone.
    pub start_tick: u32,
    /// Where it ends: its last kill, or for a clutch the round's end.
    pub end_tick: u32,
    /// The kills it is made of (indices into the round's [`RoundSummary::kills`]).
    pub kills: Vec<usize>,
}

/// Reads the whole demo once and sums it up.
pub fn summarize(data: &[u8]) -> Result<DemoSummary, TrackError> {
    let mut reader: Option<Reader> = None;
    let mut summary = DemoSummary {
        map: String::new(),
        server: String::new(),
        tick_interval: 1.0 / 64.0,
        players: Vec::new(),
        rounds: Vec::new(),
        highlights: Vec::new(),
    };
    let mut players = Players::default();
    // Whether the last round is still running (it has no end yet).
    let mut open = false;
    // The rules entity's round end count at the previous tick.
    let mut end_count: Option<u64> = None;
    // Whether the last round is decided and its teams are still to read: on every tick until the next round starts
    // (the score can land a tick after the winner), and on after the match has ended.
    let mut decided = false;
    walk_rounds(data, |view| {
        let reader = reader.get_or_insert_with(|| {
            summary.map = view.parser.header().map_name().to_owned();
            summary.server = view.parser.header().server_name().to_owned();
            summary.tick_interval = view.parser.tick_interval().unwrap_or(1.0 / 64.0);
            Reader::find(view.parser)
        });
        let starts = view
            .rounds
            .iter()
            .any(|e| matches!(e, RoundEvent::Started(_)));
        // Before this tick's round starts and ends: the match's last round is decided on the tick the match ends.
        if let Some((count, winner, reason)) = reader.round_end(view.parser, view.paths) {
            if end_count.is_some_and(|c| c != count)
                && open
                && let Some(round) = summary.rounds.last_mut()
                && round.decide(
                    Side::of_team(winner as u32),
                    RoundEnd::of_reason(reason),
                    view.tick,
                )
            {
                decided = true;
            }
            end_count = Some(count);
        }
        // Not on a new round's first tick, which already has the sides swapped at half time.
        if decided
            && !starts
            && let Some(round) = summary.rounds.last_mut()
        {
            reader.teams(view.parser, round);
        }
        for event in view.rounds {
            match *event {
                RoundEvent::Ended(info) => {
                    if let Some(round) = summary.rounds.last_mut().filter(|r| r.round == info.round)
                    {
                        round.end_tick = info.end_tick;
                    }
                    open = false;
                    decided &= !starts;
                }
                RoundEvent::Started(number) => {
                    summary.rounds.push(RoundSummary {
                        round: number,
                        start_tick: view.tick,
                        freeze_end_tick: None,
                        decided_tick: None,
                        end_tick: view.tick,
                        winner: None,
                        reason: None,
                        restarted: false,
                        t: Team::default(),
                        ct: Team::default(),
                        players: reader.roster(view.parser, view.paths, &mut players),
                        kills: Vec::new(),
                    });
                    open = true;
                }
            }
        }
        if open && let (Some(round), Some(current)) = (summary.rounds.last_mut(), view.current) {
            round.end_tick = current.end_tick;
        }
        let rounds = summary.rounds.len();
        for event in view.events {
            // The events read since the previous tick can still belong to the round that just ended; those after the
            // match's last round are left out.
            let Some(at) = summary
                .rounds
                .iter()
                .rposition(|r| r.start_tick <= event.tick)
            else {
                continue;
            };
            // An event after a round's last sampled tick is the round's only when it came with the next round's
            // start (a tick without entity changes in between); one read during a warmup after it is not.
            let with_next = summary
                .rounds
                .get(at + 1)
                .is_some_and(|next| next.start_tick == view.tick);
            let round = &mut summary.rounds[at];
            if event.tick > round.end_tick && !with_next && !(open && at + 1 == rounds) {
                continue;
            }
            match event.name.as_str() {
                "player_death" => {
                    let kill = reader.kill(view.parser, view.paths, event, &mut players);
                    round.kills.push(kill);
                }
                "round_freeze_end" if round.freeze_end_tick.is_none() => {
                    round.freeze_end_tick = Some(event.tick);
                    round.players = reader.roster(view.parser, view.paths, &mut players);
                }
                "round_end" => {
                    let winner = event.int("winner").and_then(|w| Side::of_team(w as u32));
                    let reason = RoundEnd::of_reason(event.int("reason").unwrap_or(0));
                    if round.decide(winner, reason, event.tick) {
                        decided |= open && at + 1 == rounds;
                    }
                }
                _ => {}
            }
        }
        true
    })?;
    mark_restarted(&mut summary.rounds);
    summary.players = players.list;
    summary.highlights = highlights(&summary.rounds);
    Ok(summary)
}

/// Marks the rounds played again: a round whose number comes again later (a restart, a backup restored) did not
/// count.
pub fn mark_restarted(rounds: &mut [RoundSummary]) {
    let mut lowest_later = u32::MAX;
    for round in rounds.iter_mut().rev() {
        round.restarted = round.round >= lowest_later;
        lowest_later = lowest_later.min(round.round);
    }
}

/// The players seen so far, by Steam id (bots, which have none, by name).
#[derive(Debug, Default)]
struct Players {
    list: Vec<Player>,
    by_key: HashMap<String, usize>,
}

impl Players {
    /// The index of a player, added if new; a known player's name is updated (players rename mid-match).
    fn index(&mut self, name: &str, steam_id: u64) -> usize {
        let key = if steam_id == 0 {
            format!("bot {name}")
        } else {
            steam_id.to_string()
        };
        if let Some(&i) = self.by_key.get(&key) {
            self.list[i].name = name.to_owned();
            return i;
        }
        self.list.push(Player {
            name: name.to_owned(),
            steam_id: steam_id.to_string(),
        });
        self.by_key.insert(key, self.list.len() - 1);
        self.list.len() - 1
    }
}

/// The field paths [`summarize`] reads beyond the round tracker's ([`Paths`]), looked up once by name (`None` for
/// one the demo lacks).
#[derive(Debug, Default)]
struct Reader {
    end_count: Option<FieldPath>,
    winner: Option<FieldPath>,
    reason: Option<FieldPath>,
    team: Option<TeamPaths>,
    /// The controller's `m_bPawnIsAlive`.
    alive: Option<FieldPath>,
}

#[derive(Debug)]
struct TeamPaths {
    number: FieldPath,
    score: FieldPath,
    clan: FieldPath,
}

/// The game rules entity's class (whose fields are under `m_pGameRules`) and a side's team entity's class.
const RULES_CLASS: &str = "CCSGameRulesProxy";
const TEAM_CLASS: &str = "CCSTeam";
const CONTROLLER_CLASS: &str = "CCSPlayerController";

impl Reader {
    fn find(parser: &DemoParser<'_>) -> Self {
        let Some(serializers) = parser.serializers() else {
            return Self::default();
        };
        let path = |class: &str, field: &str| {
            serializers
                .by_name(class)
                .and_then(|s| serializers.path_of(s, field))
        };
        Self {
            end_count: path(RULES_CLASS, "m_pGameRules.m_nRoundEndCount"),
            winner: path(RULES_CLASS, "m_pGameRules.m_iRoundEndWinnerTeam"),
            reason: path(RULES_CLASS, "m_pGameRules.m_eRoundEndReason"),
            team: (|| {
                Some(TeamPaths {
                    number: path(TEAM_CLASS, "m_iTeamNum")?,
                    score: path(TEAM_CLASS, "m_iScore")?,
                    clan: path(TEAM_CLASS, "m_szClanTeamname")?,
                })
            })(),
            alive: path(CONTROLLER_CLASS, "m_bPawnIsAlive"),
        }
    }

    /// The rules entity's round end count (0 while never sent), last winner's team number and reason; `None` when
    /// the demo has no such count.
    fn round_end(&self, parser: &DemoParser<'_>, paths: &Paths) -> Option<(u64, i64, i64)> {
        let count = self.end_count.as_ref()?;
        let rules = parser
            .entities()
            .iter()
            .find(|e| parser.class_name(e.class_id) == Some(paths.rules_class.as_str()))?;
        let int = |path: &Option<FieldPath>| {
            path.as_ref()
                .and_then(|p| rules.get(p))
                .and_then(FieldValue::as_i64)
                .unwrap_or(0)
        };
        Some((
            rules.get(count).and_then(FieldValue::as_u64).unwrap_or(0),
            int(&self.winner),
            int(&self.reason),
        ))
    }

    /// Writes both sides' clan names and scores into `round`.
    fn teams(&self, parser: &DemoParser<'_>, round: &mut RoundSummary) {
        let Some(paths) = &self.team else { return };
        for team in parser
            .entities()
            .iter()
            .filter(|e| parser.class_name(e.class_id) == Some(TEAM_CLASS))
        {
            let number = team
                .get(&paths.number)
                .and_then(FieldValue::as_u64)
                .unwrap_or(0);
            let side = match Side::of_team(number as u32) {
                Some(Side::T) => &mut round.t,
                Some(Side::Ct) => &mut round.ct,
                None => continue,
            };
            side.name = team
                .get(&paths.clan)
                .and_then(FieldValue::as_str)
                .unwrap_or_default()
                .to_owned();
            side.score = team
                .get(&paths.score)
                .and_then(FieldValue::as_i64)
                .unwrap_or(0) as i32;
        }
    }

    /// A controller's player and side; `None` for spectators (casters, the GOTV bot), who are not added.
    fn player(
        &self,
        parser: &DemoParser<'_>,
        paths: &Paths,
        controller: &Entity,
        players: &mut Players,
    ) -> Option<(usize, Side)> {
        let team = controller
            .get(&paths.team)
            .and_then(FieldValue::as_u64)
            .unwrap_or(0);
        let side = Side::of_team(team as u32)?;
        let name = controller
            .get(&paths.name)
            .and_then(FieldValue::as_str)
            .unwrap_or_default();
        let steam_id = controller
            .get(&paths.steam_id)
            .and_then(FieldValue::as_u64)
            .unwrap_or(0);
        debug_assert_eq!(
            parser.class_name(controller.class_id),
            Some(paths.controller_class.as_str())
        );
        Some((players.index(name, steam_id), side))
    }

    /// The controller entity at `index`, if it is one.
    fn controller<'e>(parser: &'e DemoParser<'_>, paths: &Paths, index: u32) -> Option<&'e Entity> {
        parser
            .entities()
            .get(index)
            .filter(|e| parser.class_name(e.class_id) == Some(paths.controller_class.as_str()))
    }

    /// Both sides' players alive now, in controller order: a player who left keeps a controller on their side, but no
    /// living pawn.
    fn roster(
        &self,
        parser: &DemoParser<'_>,
        paths: &Paths,
        players: &mut Players,
    ) -> Vec<RoundPlayer> {
        parser
            .entities()
            .iter()
            .filter(|e| parser.class_name(e.class_id) == Some(paths.controller_class.as_str()))
            .filter(|c| {
                self.alive
                    .as_ref()
                    .and_then(|p| c.get(p))
                    .and_then(FieldValue::as_bool)
                    != Some(false)
            })
            .filter_map(|c| self.player(parser, paths, c, players))
            .map(|(player, side)| RoundPlayer { player, side })
            .collect()
    }

    /// The kill of a `player_death` event.
    fn kill(
        &self,
        parser: &DemoParser<'_>,
        paths: &Paths,
        event: &GameEvent,
        players: &mut Players,
    ) -> Kill {
        let mut who = |key: &str| {
            let controller = Self::controller(parser, paths, event.controller(key)?)?;
            self.player(parser, paths, controller, players)
        };
        let victim = who("userid");
        let attacker = who("attacker");
        let assister = who("assister");
        let weapon = event.string("weapon").unwrap_or_default().to_owned();
        let flag = |key: &str| event.bool(key).unwrap_or(false);
        Kill {
            tick: event.tick,
            victim: victim.map(|v| v.0),
            attacker: attacker.map(|a| a.0),
            assister: assister.map(|a| a.0),
            weapon_class: WeaponClass::of(&weapon),
            weapon,
            headshot: flag("headshot"),
            wallbang: flag("penetrated"),
            noscope: flag("noscope"),
            through_smoke: flag("thrusmoke"),
            blind: flag("attackerblind"),
            airborne: flag("attackerinair"),
            flash_assist: flag("assistedflash"),
            team_kill: matches!((attacker, victim), (Some((a, sa)), Some((v, sv))) if a != v && sa == sv),
        }
    }
}

/// Kills by one player in one round that make a [`HighlightKind::MultiKill`].
pub const MULTI_KILL: usize = 3;

/// Shotguns: several pellets of one shot can kill two players, which is no collateral.
const SHOTGUNS: [&str; 4] = ["nova", "xm1014", "sawedoff", "mag7"];

/// The highlights of `rounds`, in round order, each round's by start tick.
pub fn highlights(rounds: &[RoundSummary]) -> Vec<Highlight> {
    let mut all = Vec::new();
    for (index, round) in rounds.iter().enumerate() {
        let mut found = Vec::new();
        found.extend(multi_kills(round));
        found.extend(clutch(round));
        let collaterals = collaterals(round);
        found.extend(special_kills(round, &collaterals));
        found.extend(collaterals);
        found.sort_by_key(|h: &Highlight| h.start_tick);
        for h in &mut found {
            h.round_index = index;
        }
        all.extend(found);
    }
    all
}

/// A highlight of the round being looked at (its index is set by [`highlights`]).
fn highlight(
    kind: HighlightKind,
    player: usize,
    count: usize,
    ticks: (u32, u32),
    kills: Vec<usize>,
) -> Highlight {
    Highlight {
        kind,
        round_index: 0,
        player,
        count: count as u32,
        tags: Vec::new(),
        start_tick: ticks.0,
        end_tick: ticks.1,
        kills,
    }
}

/// The kills that count for each player (of opponents), by player, in order.
fn kills_by_player(round: &RoundSummary) -> Vec<(usize, Vec<usize>)> {
    let mut by: Vec<(usize, Vec<usize>)> = Vec::new();
    for (i, kill) in round.kills.iter().enumerate() {
        let Some(attacker) = kill.attacker.filter(|_| kill.of_opponent()) else {
            continue;
        };
        match by.iter_mut().find(|(p, _)| *p == attacker) {
            Some((_, kills)) => kills.push(i),
            None => by.push((attacker, vec![i])),
        }
    }
    by
}

fn multi_kills(round: &RoundSummary) -> Vec<Highlight> {
    kills_by_player(round)
        .into_iter()
        .filter(|(_, kills)| kills.len() >= MULTI_KILL)
        .map(|(player, kills)| {
            let first = round.kills[kills[0]].tick;
            let last = round.kills[kills[kills.len() - 1]].tick;
            highlight(
                HighlightKind::MultiKill,
                player,
                kills.len(),
                (first, last),
                kills,
            )
        })
        .collect()
}

/// The round's won clutch: the first time the winning side is down to one player against at least one opponent,
/// before the round is decided (a side alone from the start is no clutch).
fn clutch(round: &RoundSummary) -> Option<Highlight> {
    let winner = round.winner?;
    let decided = round.decided_tick.unwrap_or(round.end_tick);
    let mut alive: HashMap<Side, Vec<usize>> = HashMap::new();
    for p in &round.players {
        alive.entry(p.side).or_default().push(p.player);
    }
    let count = |alive: &HashMap<Side, Vec<usize>>, side| alive.get(&side).map_or(0, Vec::len);
    if count(&alive, winner) <= 1 {
        return None;
    }
    for (i, kill) in round.kills.iter().enumerate() {
        if kill.tick > decided {
            return None;
        }
        let Some(victim) = kill.victim else { continue };
        for players in alive.values_mut() {
            players.retain(|&p| p != victim);
        }
        let opponents = count(&alive, winner.other());
        if count(&alive, winner) == 1 && opponents >= 1 {
            let player = alive[&winner][0];
            let kills: Vec<usize> = (i + 1..round.kills.len())
                .filter(|&k| {
                    let k = &round.kills[k];
                    k.attacker == Some(player) && k.of_opponent() && k.tick <= decided
                })
                .collect();
            return Some(highlight(
                HighlightKind::Clutch,
                player,
                opponents,
                (kill.tick, decided),
                kills,
            ));
        }
        if count(&alive, winner) == 0 {
            return None;
        }
    }
    None
}

/// Two or more kills by one player at the same tick with one gun: one bullet through several players.
fn collaterals(round: &RoundSummary) -> Vec<Highlight> {
    let mut found = Vec::new();
    for (player, kills) in kills_by_player(round) {
        let mut at = 0;
        while at < kills.len() {
            let first = &round.kills[kills[at]];
            let same: Vec<usize> = kills[at..]
                .iter()
                .copied()
                .take_while(|&k| {
                    round.kills[k].tick == first.tick && round.kills[k].weapon == first.weapon
                })
                .collect();
            at += same.len();
            let bullet = !matches!(
                first.weapon_class,
                WeaponClass::Grenade | WeaponClass::Knife | WeaponClass::Other
            ) && !SHOTGUNS.contains(&first.weapon.as_str());
            if bullet && same.len() >= 2 {
                let ticks = (first.tick, first.tick);
                found.push(highlight(
                    HighlightKind::Collateral,
                    player,
                    same.len(),
                    ticks,
                    same,
                ));
            }
        }
    }
    found
}

/// The tags of a kill that make it a special kill.
pub fn kill_tags(kill: &Kill) -> Vec<KillTag> {
    [
        (kill.noscope, KillTag::Noscope),
        (kill.wallbang, KillTag::Wallbang),
        (kill.through_smoke, KillTag::ThroughSmoke),
        (kill.airborne, KillTag::Airborne),
        (kill.blind, KillTag::Blind),
        (kill.weapon_class == WeaponClass::Knife, KillTag::Knife),
    ]
    .into_iter()
    .filter_map(|(on, tag)| on.then_some(tag))
    .collect()
}

/// The special kills of a round. A collateral's later kills went through a player, which the game counts as a
/// penetration: they are no wallbang for that.
fn special_kills(round: &RoundSummary, collaterals: &[Highlight]) -> Vec<Highlight> {
    let behind: Vec<usize> = collaterals
        .iter()
        .flat_map(|h| h.kills.iter().skip(1).copied())
        .collect();
    let mut found = Vec::new();
    for (i, kill) in round.kills.iter().enumerate() {
        let Some(attacker) = kill.attacker.filter(|_| kill.of_opponent()) else {
            continue;
        };
        let mut tags = kill_tags(kill);
        if behind.contains(&i) {
            tags.retain(|t| *t != KillTag::Wallbang);
        }
        if !tags.is_empty() {
            let ticks = (kill.tick, kill.tick);
            found.push(Highlight {
                tags,
                ..highlight(HighlightKind::SpecialKill, attacker, 1, ticks, vec![i])
            });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kill of `victim` by `attacker` at `tick` with `weapon`, no flags.
    fn kill(tick: u32, attacker: usize, victim: usize, weapon: &str) -> Kill {
        Kill {
            tick,
            victim: Some(victim),
            attacker: Some(attacker),
            assister: None,
            weapon: weapon.to_owned(),
            weapon_class: WeaponClass::of(weapon),
            headshot: false,
            wallbang: false,
            noscope: false,
            through_smoke: false,
            blind: false,
            airborne: false,
            flash_assist: false,
            team_kill: false,
        }
    }

    /// A round won by `winner`: players 0–4 are T, 5–9 CT.
    fn round(winner: Side, kills: Vec<Kill>) -> RoundSummary {
        RoundSummary {
            round: 1,
            start_tick: 0,
            freeze_end_tick: Some(100),
            decided_tick: Some(4000),
            end_tick: 5000,
            winner: Some(winner),
            reason: Some(RoundEnd::Eliminated),
            restarted: false,
            t: Team::default(),
            ct: Team::default(),
            players: (0..10)
                .map(|player| RoundPlayer {
                    player,
                    side: if player < 5 { Side::T } else { Side::Ct },
                })
                .collect(),
            kills,
        }
    }

    #[test]
    fn weapon_classes() {
        assert_eq!(WeaponClass::of("awp"), WeaponClass::Sniper);
        assert_eq!(WeaponClass::of("knife_karambit"), WeaponClass::Knife);
        assert_eq!(WeaponClass::of("bayonet"), WeaponClass::Knife);
        assert_eq!(WeaponClass::of("usp_silencer"), WeaponClass::Pistol);
        assert_eq!(WeaponClass::of("inferno"), WeaponClass::Grenade);
        assert_eq!(WeaponClass::of("world"), WeaponClass::Other);
    }

    #[test]
    fn an_ace_is_a_five_kill_multi_kill() {
        let kills = (5..10)
            .map(|v| kill(1000 + v as u32 * 10, 0, v, "ak47"))
            .collect();
        let found = highlights(&[round(Side::T, kills)]);
        assert_eq!(found.len(), 1);
        let ace = &found[0];
        assert_eq!(
            (ace.kind, ace.player, ace.count),
            (HighlightKind::MultiKill, 0, 5)
        );
        assert_eq!((ace.start_tick, ace.end_tick), (1050, 1090));
        assert_eq!(ace.kills, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn team_kills_and_suicides_do_not_count() {
        let mut team_kill = kill(1000, 0, 1, "ak47");
        team_kill.team_kill = true;
        let kills = vec![
            team_kill,
            kill(1100, 0, 0, "hegrenade"),
            kill(1200, 0, 5, "ak47"),
            kill(1300, 0, 6, "ak47"),
        ];
        assert!(highlights(&[round(Side::Ct, kills)]).is_empty());
    }

    #[test]
    fn a_won_one_versus_three() {
        // T loses four players (0–3) while killing two CTs, then T 4 kills the last three CTs.
        let kills = vec![
            kill(1000, 5, 0, "m4a1"),
            kill(1100, 1, 5, "ak47"),
            kill(1200, 6, 1, "m4a1"),
            kill(1300, 6, 2, "m4a1"),
            kill(1400, 2, 6, "ak47"), // impossible order on purpose: the dead do not matter for the count
            kill(1500, 7, 3, "m4a1"),
            kill(1600, 4, 7, "ak47"),
            kill(1700, 4, 8, "ak47"),
            kill(1800, 4, 9, "deagle"),
        ];
        let found = highlights(&[round(Side::T, kills)]);
        let clutch = found
            .iter()
            .find(|h| h.kind == HighlightKind::Clutch)
            .expect("a clutch");
        assert_eq!((clutch.player, clutch.count), (4, 3));
        assert_eq!(
            (clutch.start_tick, clutch.end_tick),
            (1500, 4000),
            "it ends when the round is decided"
        );
        assert_eq!(clutch.kills, vec![6, 7, 8]);
        // T 4's three kills are a multi-kill too.
        assert!(
            found
                .iter()
                .any(|h| h.kind == HighlightKind::MultiKill && h.player == 4)
        );
    }

    #[test]
    fn a_lost_clutch_is_no_highlight() {
        let kills = (0..4)
            .map(|v| kill(1000 + v as u32 * 10, 5, v, "m4a1"))
            .chain([kill(2000, 6, 4, "m4a1")]);
        let found = highlights(&[round(Side::Ct, kills.collect())]);
        assert!(found.iter().all(|h| h.kind != HighlightKind::Clutch));
    }

    #[test]
    fn one_bullet_two_kills_is_a_collateral() {
        let kills = vec![
            kill(1000, 9, 0, "awp"),
            kill(1000, 9, 1, "awp"),
            kill(1000, 8, 2, "hegrenade"),
            kill(1000, 8, 3, "hegrenade"),
        ];
        let found = highlights(&[round(Side::Ct, kills)]);
        let collaterals: Vec<_> = found
            .iter()
            .filter(|h| h.kind == HighlightKind::Collateral)
            .collect();
        assert_eq!(
            collaterals.len(),
            1,
            "a grenade's two kills are no collateral"
        );
        assert_eq!((collaterals[0].player, collaterals[0].count), (9, 2));
    }

    #[test]
    fn special_kills_carry_their_tags() {
        let mut noscope = kill(1000, 9, 0, "awp");
        noscope.noscope = true;
        noscope.wallbang = true;
        let kills = vec![
            noscope,
            kill(1100, 8, 1, "knife_t"),
            kill(1200, 8, 2, "ak47"),
        ];
        let found = highlights(&[round(Side::Ct, kills)]);
        let tags: Vec<_> = found.iter().map(|h| (h.player, h.tags.clone())).collect();
        assert_eq!(
            tags,
            vec![
                (9, vec![KillTag::Noscope, KillTag::Wallbang]),
                (8, vec![KillTag::Knife])
            ]
        );
    }

    #[test]
    fn kills_after_the_round_is_decided_make_no_clutch() {
        // The CTs defuse (decided at 4000) with two alive against one T, who then kills one of them.
        let kills: Vec<Kill> = (0..4)
            .map(|v| kill(1000 + v as u32 * 10, 5, v, "m4a1"))
            .chain((5..8).map(|v| kill(2000 + v as u32 * 10, 4, v, "ak47")))
            .chain([kill(4100, 4, 8, "ak47")])
            .collect();
        let found = highlights(&[round(Side::Ct, kills)]);
        assert!(found.iter().all(|h| h.kind != HighlightKind::Clutch));
    }

    #[test]
    fn a_collateral_s_second_kill_is_no_wallbang() {
        let mut behind = kill(1000, 9, 1, "awp");
        behind.wallbang = true;
        let found = highlights(&[round(Side::Ct, vec![kill(1000, 9, 0, "awp"), behind])]);
        let kinds: Vec<_> = found.iter().map(|h| h.kind).collect();
        assert_eq!(kinds, [HighlightKind::Collateral]);
    }

    #[test]
    fn only_the_first_decisive_end_counts() {
        let mut r = round(Side::T, Vec::new());
        r.decided_tick = None;
        assert!(
            !r.decide(None, RoundEnd::Other, 3000),
            "a restart decides nothing"
        );
        assert!(r.decide(Some(Side::Ct), RoundEnd::BombDefused, 3500));
        assert!(!r.decide(Some(Side::T), RoundEnd::Eliminated, 3600));
        assert_eq!(
            (r.winner, r.reason, r.decided_tick),
            (Some(Side::Ct), Some(RoundEnd::BombDefused), Some(3500))
        );
    }

    #[test]
    fn shotgun_pellets_are_no_collateral() {
        let kills = vec![kill(1000, 9, 0, "nova"), kill(1000, 9, 1, "nova")];
        let found = highlights(&[round(Side::Ct, kills)]);
        assert!(found.iter().all(|h| h.kind != HighlightKind::Collateral));
    }

    #[test]
    fn a_kill_without_a_known_victim_does_not_count() {
        let mut kills: Vec<Kill> = (5..7)
            .map(|v| kill(1000 + v as u32, 0, v, "ak47"))
            .collect();
        let mut unknown = kill(1100, 0, 0, "ak47");
        unknown.victim = None;
        kills.push(unknown);
        assert!(highlights(&[round(Side::T, kills)]).is_empty());
    }

    #[test]
    fn highlights_point_at_their_round_by_index() {
        let ace: Vec<Kill> = (5..10)
            .map(|v| kill(1000 + v as u32, 0, v, "ak47"))
            .collect();
        let quiet = round(Side::T, Vec::new());
        let found = highlights(&[quiet.clone(), quiet, round(Side::T, ace)]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].round_index, 2);
    }

    #[test]
    fn a_round_played_again_is_marked_restarted() {
        // Rounds 1, 2, then a restart: 1, 2, 3.
        let mut rounds: Vec<RoundSummary> = [1, 2, 1, 2, 3]
            .into_iter()
            .map(|number| RoundSummary {
                round: number,
                ..round(Side::T, Vec::new())
            })
            .collect();
        mark_restarted(&mut rounds);
        let restarted: Vec<bool> = rounds.iter().map(|r| r.restarted).collect();
        assert_eq!(restarted, [true, true, false, false, false]);
    }

    #[test]
    fn round_end_reasons() {
        assert_eq!(RoundEnd::of_reason(1), RoundEnd::BombExploded);
        assert_eq!(RoundEnd::of_reason(7), RoundEnd::BombDefused);
        assert_eq!(RoundEnd::of_reason(9), RoundEnd::Eliminated);
        assert_eq!(RoundEnd::of_reason(12), RoundEnd::TimeRanOut);
    }

    #[test]
    fn players_are_kept_by_steam_id_and_bots_by_name() {
        let mut players = Players::default();
        assert_eq!(players.index("a", 7), 0);
        assert_eq!(players.index("bot", 0), 1);
        assert_eq!(players.index("renamed", 7), 0);
        assert_eq!(players.list[0].name, "renamed");
        assert_eq!(players.index("other bot", 0), 2);
    }
}
