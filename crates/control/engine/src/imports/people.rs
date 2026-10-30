//! The people of a source: who each is here, and whom a thing of the source is written by.
//!
//! A person of a package is taken for an account here in this order: the account that has the same identifier (the package is
//! of this deployment, or of a copy of it), the account whose confirmed address is theirs, the account with their username, and
//! what the person chose in the review, which wins over all three. Anybody left is an imported person: a name, not an account,
//! who cannot sign in. Putting something under the name of an account is a thing an Admin of the place may do; for anybody else
//! the importer writes it and the name of whoever wrote it there stays beside it as "originally by".

use orch_api_contract::imports::{ImportAuthors, ImportPersonChoice, ImportPersonMatch};
use orch_api_contract::package::PackagePrincipal;
use orch_db::queries::imports as rows;
use orch_db::{Conn, DbError};
use orch_types::Id;
use std::collections::{HashMap, HashSet};

/// What an imported person is called where the source does not say.
pub const IMPORTED_PERSON: &str = "Imported person";

/// A person of the source and whom they are taken for.
#[derive(Debug, Clone, PartialEq)]
pub struct Matched {
    /// The person of the source.
    pub principal: PackagePrincipal,
    /// How the match was made.
    pub how: ImportPersonMatch,
    /// The account here, if there is one.
    pub user: Option<Id>,
}

/// Takes the people of a source for accounts here.
pub async fn match_people(
    conn: &mut Conn,
    principals: &[PackagePrincipal],
    choices: &[ImportPersonChoice],
) -> Result<Vec<Matched>, DbError> {
    let chosen: HashMap<Id, Option<Id>> = choices
        .iter()
        .map(|choice| (choice.principal, choice.user))
        .collect();
    let ids: Vec<Id> = principals.iter().map(|each| each.id).collect();
    let same_id: HashSet<Id> = rows::existing_users(conn, &ids)
        .await?
        .into_iter()
        .collect();
    let emails: Vec<String> = principals
        .iter()
        .filter(|each| each.email_verified)
        .filter_map(|each| each.email.as_deref().map(str::to_lowercase))
        .collect();
    let by_email: HashMap<String, Id> = if emails.is_empty() {
        HashMap::new()
    } else {
        rows::users_by_confirmed_email(conn, &emails)
            .await?
            .into_iter()
            .collect()
    };
    let names: Vec<String> = principals
        .iter()
        .map(|each| each.username.to_lowercase())
        .collect();
    let by_name: HashMap<String, Id> = rows::users_by_username(conn, &names)
        .await?
        .into_iter()
        .collect();
    // An account named in a choice has to be one: a choice of somebody who is not there is no choice.
    let picked: Vec<Id> = chosen.values().flatten().copied().collect();
    let picked_there: HashSet<Id> = if picked.is_empty() {
        HashSet::new()
    } else {
        rows::existing_users(conn, &picked)
            .await?
            .into_iter()
            .collect()
    };
    Ok(principals
        .iter()
        .map(|principal| {
            let (how, user) = if let Some(choice) = chosen.get(&principal.id) {
                match choice {
                    Some(user) if picked_there.contains(user) => {
                        (ImportPersonMatch::Choice, Some(*user))
                    }
                    _ => (ImportPersonMatch::None, None),
                }
            } else if same_id.contains(&principal.id) {
                (ImportPersonMatch::Id, Some(principal.id))
            } else if let Some(user) = principal
                .email
                .as_deref()
                .filter(|_| principal.email_verified)
                .and_then(|email| by_email.get(&email.to_lowercase()))
            {
                (ImportPersonMatch::Email, Some(*user))
            } else if let Some(user) = by_name.get(&principal.username.to_lowercase()) {
                (ImportPersonMatch::Username, Some(*user))
            } else {
                (ImportPersonMatch::None, None)
            };
            Matched {
                principal: principal.clone(),
                how,
                user,
            }
        })
        .collect())
}

/// Whom a thing of the source is written by here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
    /// The account that is its author here.
    pub user: Id,
    /// The name of whoever wrote it there, where that is not the account: shown as "originally by".
    pub original: Option<String>,
}

/// The people of a source as an import writes them.
#[derive(Debug, Clone)]
pub struct Credits {
    importer: Id,
    authors: ImportAuthors,
    people: HashMap<Id, Matched>,
}

impl Credits {
    /// The people `matched`, written as `authors` says by the person `importer`.
    #[must_use]
    pub fn new(importer: Id, authors: ImportAuthors, matched: Vec<Matched>) -> Self {
        Self {
            importer,
            authors,
            people: matched
                .into_iter()
                .map(|each| (each.principal.id, each))
                .collect(),
        }
    }

    /// Whom something written by `principal` there is written by here.
    #[must_use]
    pub fn of(&self, principal: Option<Id>) -> Credit {
        let Some(principal) = principal else {
            return Credit {
                user: self.importer,
                original: None,
            };
        };
        let found = self.people.get(&principal);
        let name = found.map_or_else(
            || IMPORTED_PERSON.to_owned(),
            |each| each.principal.display_name.clone(),
        );
        match found.and_then(|each| each.user) {
            Some(user) if user == self.importer => Credit {
                user,
                original: None,
            },
            Some(user) if self.authors == ImportAuthors::Accounts => Credit {
                user,
                original: None,
            },
            _ => Credit {
                user: self.importer,
                original: Some(name),
            },
        }
    }

    /// Applies the same attribution rule to the author carried by a document or one of its blocks.
    #[must_use]
    pub fn author(
        &self,
        author: Option<&orch_api_contract::documents::Author>,
        fallback: Option<Id>,
    ) -> Credit {
        use orch_api_contract::documents::Author;
        match author {
            Some(Author::User(user)) => self.of(Some(user.id)),
            Some(Author::Agent(agent)) => Credit {
                user: self.importer,
                original: Some(agent.agent.clone()),
            },
            None => self.of(fallback),
        }
    }

    /// The account a mention of `principal` names here, where there is one: a mention is a reference, not a claim of
    /// authorship, so it follows the match whoever the import writes things as.
    #[must_use]
    pub fn mention(&self, principal: Id) -> Option<Id> {
        self.people.get(&principal).and_then(|each| each.user)
    }

    /// The name of a person of the source, to write where an account cannot be named.
    #[must_use]
    pub fn name(&self, principal: Id) -> String {
        self.people.get(&principal).map_or_else(
            || IMPORTED_PERSON.to_owned(),
            |each| each.principal.display_name.clone(),
        )
    }

    /// The username of a person of the source.
    #[must_use]
    pub fn username(&self, principal: Id) -> Option<&str> {
        self.people
            .get(&principal)
            .map(|each| each.principal.username.as_str())
    }

    /// The account a person of the source is taken for, whoever the import writes things as.
    #[must_use]
    pub fn account(&self, principal: Id) -> Option<Id> {
        self.people.get(&principal).and_then(|each| each.user)
    }

    /// Whoever imports.
    #[must_use]
    pub const fn importer(&self) -> Id {
        self.importer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::users::UserKind;
    use orch_types::new_id;

    fn principal(name: &str) -> PackagePrincipal {
        PackagePrincipal {
            id: new_id(),
            kind: UserKind::Human,
            username: name.to_owned(),
            display_name: name.to_uppercase(),
            email: None,
            email_verified: false,
        }
    }

    fn matched(principal: PackagePrincipal, user: Option<Id>) -> Matched {
        Matched {
            principal,
            how: if user.is_some() {
                ImportPersonMatch::Username
            } else {
                ImportPersonMatch::None
            },
            user,
        }
    }

    #[test]
    fn an_admin_may_write_things_as_the_accounts_the_people_are_taken_for() {
        let (importer, account) = (new_id(), new_id());
        let ana = principal("ana");
        let credits = Credits::new(
            importer,
            ImportAuthors::Accounts,
            vec![matched(ana.clone(), Some(account))],
        );
        assert_eq!(
            credits.of(Some(ana.id)),
            Credit {
                user: account,
                original: None
            }
        );
    }

    #[test]
    fn anybody_else_writes_the_things_and_the_name_that_wrote_them_there_stays() {
        let (importer, account) = (new_id(), new_id());
        let (ana, bo) = (principal("ana"), principal("bo"));
        let credits = Credits::new(
            importer,
            ImportAuthors::Labels,
            vec![
                matched(ana.clone(), Some(account)),
                matched(bo.clone(), None),
            ],
        );
        for who in [&ana, &bo] {
            let credit = credits.of(Some(who.id));
            assert_eq!(credit.user, importer);
            assert_eq!(credit.original, Some(who.display_name.clone()));
        }
        assert_eq!(
            credits.of(Some(new_id())).original.as_deref(),
            Some(IMPORTED_PERSON),
            "somebody the source does not name is an imported person"
        );
        assert_eq!(
            credits.of(None).original,
            None,
            "a system thing has no author to name"
        );
    }

    #[test]
    fn the_importer_is_never_named_as_the_one_who_wrote_what_they_wrote_themself() {
        let importer = new_id();
        let me = principal("me");
        for authors in [ImportAuthors::Labels, ImportAuthors::Accounts] {
            let credits =
                Credits::new(importer, authors, vec![matched(me.clone(), Some(importer))]);
            assert_eq!(credits.of(Some(me.id)).original, None);
        }
    }

    #[test]
    fn a_mention_follows_the_match_whoever_writes_the_things() {
        let (importer, account) = (new_id(), new_id());
        let ana = principal("ana");
        let credits = Credits::new(
            importer,
            ImportAuthors::Labels,
            vec![matched(ana.clone(), Some(account))],
        );
        assert_eq!(credits.mention(ana.id), Some(account));
        assert_eq!(credits.mention(new_id()), None);
    }
}
