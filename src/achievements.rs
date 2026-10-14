//! A game's RetroAchievements, and which of them this user has.
//!
//! Two of RomM's answers make it: its detail of the game for the list, and the user's own record
//! (`/api/users/me`) for the progress of the RetroAchievements account linked to them. An earned
//! achievement is named there only by its badge id, so that is what the two are matched by.
//!
//! A badge is not an achievement, though: a game can give several achievements the same picture
//! (Ape Escape: 68 achievements, five earned, eleven sharing those five badges). So a badge only
//! one achievement wears says which one was earned; one shared by several says it only when all of
//! them or none of them are — in between, the achievement is drawn neither earned nor locked rather
//! than guessed. The count is RetroAchievements' own, which is exact either way.

use std::collections::HashMap;

use crate::romm::models::{Achievement, EarnedAchievement, UserInfo};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameAchievements {
    /// In the game's own order.
    pub list: Vec<Achievement>,
    /// What this user has earned of them, by badge id — several to a badge when achievements
    /// share one.
    earned: HashMap<String, Vec<EarnedAchievement>>,
    /// How many of the game's achievements wear each badge.
    wearing: HashMap<String, usize>,
    /// RetroAchievements' own count of what this user has earned.
    awarded: Option<usize>,
    /// Whether a RetroAchievements account is linked to this RomM user: without one nothing can be
    /// earned, and the page says how to link one rather than drawing every badge locked. None when
    /// the user's record could not be read, and earning is simply not shown.
    pub linked: Option<bool>,
}

impl GameAchievements {
    /// `list` from RomM's detail of the game, `user` its record of this user, `ra_id` the game's
    /// RetroAchievements id.
    pub fn new(mut list: Vec<Achievement>, user: Option<&UserInfo>, ra_id: Option<i64>) -> Self {
        list.sort_by_key(|a| a.display_order.unwrap_or(i64::MAX));
        let progress = user
            .and_then(|u| u.ra_progression.as_ref())
            .and_then(|p| p.results.iter().find(|g| g.rom_ra_id.is_some() && g.rom_ra_id == ra_id));
        let mut earned: HashMap<String, Vec<EarnedAchievement>> = HashMap::new();
        for entry in progress.map(|g| g.earned_achievements.as_slice()).unwrap_or_default() {
            earned.entry(entry.id.clone()).or_default().push(entry.clone());
        }
        let mut wearing: HashMap<String, usize> = HashMap::new();
        for badge in list.iter().filter_map(|a| a.badge_id.clone()) {
            *wearing.entry(badge).or_default() += 1;
        }
        let awarded = progress.and_then(|g| g.num_awarded).and_then(|n| usize::try_from(n).ok());
        let linked = user.map(|u| u.ra_username.as_deref().is_some_and(|n| !n.trim().is_empty()));
        Self { list, earned, wearing, awarded, linked }
    }

    /// How many of `achievement`'s badge this user has earned, and how many achievements wear it.
    fn tally(&self, achievement: &Achievement) -> Option<(&[EarnedAchievement], usize)> {
        let badge = achievement.badge_id.as_deref()?;
        let earned = self.earned.get(badge).map(Vec::as_slice).unwrap_or_default();
        Some((earned, self.wearing.get(badge).copied().unwrap_or(1)))
    }

    /// When this user earned `achievement` — known only when every achievement wearing its badge
    /// was earned. The n-th of those achievements takes the n-th date, so a shared badge's dates
    /// stay spread out rather than all repeating the first.
    pub fn earned(&self, achievement: &Achievement) -> Option<&EarnedAchievement> {
        let (earned, wearing) = self.tally(achievement)?;
        if earned.is_empty() || earned.len() < wearing {
            return None;
        }
        let badge = achievement.badge_id.as_deref()?;
        let nth = self.list.iter().filter(|a| a.badge_id.as_deref() == Some(badge)).position(|a| std::ptr::eq(a, achievement)).unwrap_or(0);
        earned.get(nth).or(earned.first())
    }

    /// Drawn locked: a linked account that has earned nothing wearing its badge.
    pub fn locked(&self, achievement: &Achievement) -> bool {
        self.linked == Some(true) && self.tally(achievement).is_none_or(|(earned, _)| earned.is_empty())
    }

    /// How many this user has: RetroAchievements' own count, or failing that what the badges say.
    pub fn earned_count(&self) -> usize {
        self.awarded.unwrap_or_else(|| self.list.iter().filter(|a| self.earned(a).is_some()).count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn achievement(badge: &str, order: i64) -> Achievement {
        Achievement { title: badge.into(), badge_id: Some(badge.into()), display_order: Some(order), ..Default::default() }
    }

    #[test]
    fn a_badge_several_achievements_share_marks_them_only_when_it_says_which() {
        let user: UserInfo = serde_json::from_value(serde_json::json!({
            "id": 1, "ra_username": "player",
            "ra_progression": { "results": [{ "rom_ra_id": 5, "num_awarded": 3, "earned_achievements": [
                { "id": "shared", "date": "2026-01-01 10:00:00" },
                { "id": "both", "date": "2026-02-01 10:00:00" },
                { "id": "both", "date": "2026-03-01 10:00:00" }
            ] }] }
        }))
        .unwrap();
        let list = vec![achievement("shared", 1), achievement("shared", 2), achievement("both", 3), achievement("both", 4), achievement("none", 5)];
        let game = GameAchievements::new(list, Some(&user), Some(5));
        let (shared_a, shared_b, both_a, both_b, none) = (&game.list[0], &game.list[1], &game.list[2], &game.list[3], &game.list[4]);
        assert!(game.earned(shared_a).is_none() && !game.locked(shared_a), "one of two: neither earned nor locked");
        assert!(game.earned(shared_b).is_none() && !game.locked(shared_b));
        assert_eq!(game.earned(both_a).and_then(|e| e.date.as_deref()), Some("2026-02-01 10:00:00"));
        assert_eq!(game.earned(both_b).and_then(|e| e.date.as_deref()), Some("2026-03-01 10:00:00"), "each takes its own date");
        assert!(game.locked(none));
        assert_eq!(game.earned_count(), 3, "RetroAchievements' own count");
    }

    #[test]
    fn what_this_user_earned_is_matched_by_badge_for_this_game_only() {
        let user: UserInfo = serde_json::from_value(serde_json::json!({
            "id": 1,
            "ra_username": "player",
            "ra_progression": { "total": 2, "results": [
                { "rom_ra_id": 319, "earned_achievements": [{ "id": "60274", "date": "2026-08-06 19:19:21" }] },
                { "rom_ra_id": 999, "earned_achievements": [{ "id": "11111", "date": "2026-01-01 00:00:00" }] }
            ] }
        }))
        .unwrap();
        let game = GameAchievements::new(vec![achievement("11111", 2), achievement("60274", 1)], Some(&user), Some(319));
        assert_eq!(game.list[0].title, "60274", "in the game's own order");
        assert_eq!(game.earned(&game.list[0]).and_then(|e| e.date.as_deref()), Some("2026-08-06 19:19:21"));
        assert!(game.locked(&game.list[1]), "another game's badge is not this one's");
        assert_eq!(game.earned_count(), 1);

        let unlinked: UserInfo = serde_json::from_value(serde_json::json!({ "id": 1 })).unwrap();
        let shown = GameAchievements::new(vec![achievement("60274", 1)], Some(&unlinked), Some(319));
        assert_eq!(shown.linked, Some(false));
        assert!(!shown.locked(&shown.list[0]), "nothing is drawn locked without an account to earn it");
        assert_eq!(GameAchievements::new(Vec::new(), None, Some(319)).linked, None);
    }
}
