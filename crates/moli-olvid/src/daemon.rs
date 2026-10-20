//! The daemon's calls Moli needs, with the numbers of the fields they use
//! (from `olvid/daemon/{command,admin,datatypes}/v1/*.proto`).

use anyhow::{Context as _, bail};

use crate::grpc::Daemon;
use crate::proto::{fields, put_str, put_uint};

const TOOL: &str = "/olvid.daemon.services.v1.ToolCommandService";
const IDENTITY_ADMIN: &str = "/olvid.daemon.services.v1.IdentityAdminService";
const KEY_ADMIN: &str = "/olvid.daemon.services.v1.ClientKeyAdminService";
const IDENTITY: &str = "/olvid.daemon.services.v1.IdentityCommandService";
const INVITATION: &str = "/olvid.daemon.services.v1.InvitationCommandService";
const CONTACT: &str = "/olvid.daemon.services.v1.ContactCommandService";
const DISCUSSION: &str = "/olvid.daemon.services.v1.DiscussionCommandService";
const MESSAGE: &str = "/olvid.daemon.services.v1.MessageCommandService";

/// `Invitation.Status`: what the bot owes the invitation now.
pub const WAIT_YOU_TO_ACCEPT: u64 = 1;
pub const WAIT_YOU_FOR_SAS: u64 = 4;
pub const WAIT_IT_FOR_SAS: u64 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invitation {
    pub id: u64,
    pub status: u64,
    pub name: String,
    /// The code this side shows, for the person to type in their Olvid.
    pub sas: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contact {
    pub id: u64,
    pub name: String,
}

/// Is the daemon there, and does it accept this key?
pub async fn ping(d: &Daemon) -> anyhow::Result<()> {
    d.call(&format!("{TOOL}/Ping"), &[]).await.map(|_| ())
}

/// A new bot identity (administrator's key). Its id.
pub async fn identity_new(admin: &Daemon, first_name: &str) -> anyhow::Result<u64> {
    let mut details = Vec::new();
    put_str(&mut details, 1, first_name);
    let mut request = Vec::new();
    crate::proto::put_bytes(&mut request, 1, &details);
    let answer = admin
        .call(&format!("{IDENTITY_ADMIN}/IdentityNew"), &request)
        .await?;
    let identity = first_field(&answer, 1, "IdentityNew")?;
    let id = fields(&identity)?
        .into_iter()
        .find(|(n, _)| *n == 1)
        .map(|(_, v)| v.uint())
        .filter(|id| *id != 0)
        .context("IdentityNew: no identity id")?;
    Ok(id)
}

/// The client key of an identity (administrator's key).
pub async fn client_key_new(admin: &Daemon, name: &str, identity: u64) -> anyhow::Result<String> {
    let mut request = Vec::new();
    put_str(&mut request, 1, name);
    put_uint(&mut request, 2, identity);
    let answer = admin
        .call(&format!("{KEY_ADMIN}/ClientKeyNew"), &request)
        .await?;
    let key = first_field(&answer, 1, "ClientKeyNew")?;
    let key = fields(&key)?
        .into_iter()
        .find(|(n, _)| *n == 2)
        .map(|(_, v)| v.text())
        .filter(|k| !k.is_empty())
        .context("ClientKeyNew: no key")?;
    Ok(key)
}

/// The link a person opens in their Olvid to meet the bot.
pub async fn invitation_link(d: &Daemon) -> anyhow::Result<String> {
    let answer = d
        .call(&format!("{IDENTITY}/IdentityGetInvitationLink"), &[])
        .await?;
    let link = answer
        .first()
        .map(|m| fields(m))
        .transpose()?
        .unwrap_or_default()
        .into_iter()
        .find(|(n, _)| *n == 1)
        .map(|(_, v)| v.text())
        .filter(|l| !l.is_empty())
        .context("IdentityGetInvitationLink: no link")?;
    Ok(link)
}

pub async fn invitations(d: &Daemon) -> anyhow::Result<Vec<Invitation>> {
    let mut out = Vec::new();
    for message in d.call(&format!("{INVITATION}/InvitationList"), &[]).await? {
        for (n, v) in fields(&message)? {
            if n != 1 {
                continue;
            }
            let mut item = Invitation {
                id: 0,
                status: 0,
                name: String::new(),
                sas: String::new(),
            };
            for (n, v) in fields(v.bytes())? {
                match n {
                    1 => item.id = v.uint(),
                    2 => item.status = v.uint(),
                    3 => item.name = v.text(),
                    5 => item.sas = v.text(),
                    _ => {}
                }
            }
            out.push(item);
        }
    }
    Ok(out)
}

pub async fn invitation_accept(d: &Daemon, id: u64) -> anyhow::Result<()> {
    let mut request = Vec::new();
    put_uint(&mut request, 1, id);
    d.call(&format!("{INVITATION}/InvitationAccept"), &request)
        .await
        .map(|_| ())
}

/// The code the other person's Olvid shows.
pub async fn invitation_sas(d: &Daemon, id: u64, sas: &str) -> anyhow::Result<()> {
    let mut request = Vec::new();
    put_uint(&mut request, 1, id);
    put_str(&mut request, 2, sas);
    d.call(&format!("{INVITATION}/InvitationSas"), &request)
        .await
        .map(|_| ())
}

pub async fn contacts(d: &Daemon) -> anyhow::Result<Vec<Contact>> {
    let mut out = Vec::new();
    for message in d.call(&format!("{CONTACT}/ContactList"), &[]).await? {
        for (n, v) in fields(&message)? {
            if n != 1 {
                continue;
            }
            let mut item = Contact {
                id: 0,
                name: String::new(),
            };
            for (n, v) in fields(v.bytes())? {
                match n {
                    1 => item.id = v.uint(),
                    2 => item.name = v.text(),
                    _ => {}
                }
            }
            out.push(item);
        }
    }
    Ok(out)
}

/// The one-to-one discussion with a contact.
pub async fn discussion_with(d: &Daemon, contact: u64) -> anyhow::Result<u64> {
    let mut request = Vec::new();
    put_uint(&mut request, 1, contact);
    let answer = d
        .call(&format!("{DISCUSSION}/DiscussionGetByContact"), &request)
        .await?;
    let discussion = first_field(&answer, 1, "DiscussionGetByContact")?;
    fields(&discussion)?
        .into_iter()
        .find(|(n, _)| *n == 1)
        .map(|(_, v)| v.uint())
        .filter(|id| *id != 0)
        .context("DiscussionGetByContact: no discussion id")
}

pub async fn send(d: &Daemon, discussion: u64, body: &str) -> anyhow::Result<()> {
    let mut request = Vec::new();
    put_uint(&mut request, 1, discussion);
    put_str(&mut request, 2, body);
    d.call(&format!("{MESSAGE}/MessageSend"), &request)
        .await
        .map(|_| ())
}

/// Field `n` (a nested message) of the first message of an answer.
fn first_field(answer: &[Vec<u8>], n: u32, call: &str) -> anyhow::Result<Vec<u8>> {
    let Some(first) = answer.first() else {
        bail!("{call}: empty answer");
    };
    fields(first)?
        .into_iter()
        .find(|(field, _)| *field == n)
        .map(|(_, v)| v.bytes().to_vec())
        .with_context(|| format!("{call}: field {n} missing"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::put_bytes;

    #[test]
    fn the_first_nested_message_is_found() {
        let mut inner = Vec::new();
        put_uint(&mut inner, 1, 42);
        let mut outer = Vec::new();
        put_bytes(&mut outer, 1, &inner);
        let found = first_field(&[outer], 1, "x").unwrap();
        assert_eq!(fields(&found).unwrap()[0].1.uint(), 42);
        assert!(first_field(&[], 1, "x").is_err());
        assert!(first_field(&[Vec::new()], 1, "x").is_err());
    }
}
