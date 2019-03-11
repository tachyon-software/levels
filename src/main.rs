extern crate serde;
extern crate serenity;
#[macro_use]
extern crate serde_derive;
extern crate chrono;
extern crate colored;
extern crate fern;
extern crate left_pad;
extern crate log;
extern crate rand;
extern crate redis;
extern crate reqwest;
extern crate serde_json;

use chrono::prelude::*;
use log::{error, info, warn};
use rand::{thread_rng, Rng};
use redis::Commands;
use serenity::client::{Client, Context};
use serenity::framework::standard::StandardFramework;
use serenity::model::{
    channel::Message,
    id::{ChannelId, RoleId, UserId},
};
use serenity::prelude::{EventHandler, TypeMapKey};
use std::collections::HashSet;
use std::{cmp, env, fs, hash, num, path, thread};

mod announce;

#[derive(Debug)]
enum QueryError {
    Redis(redis::RedisError),
    Serde(serde_json::error::Error),
}

impl From<redis::RedisError> for QueryError {
    fn from(e: redis::RedisError) -> QueryError {
        QueryError::Redis(e)
    }
}

impl From<serde_json::error::Error> for QueryError {
    fn from(e: serde_json::error::Error) -> QueryError {
        QueryError::Serde(e)
    }
}

struct Handler;
impl Handler {
    fn get_users(db: &redis::Client) -> Result<Vec<XPUser>, QueryError> {
        let con = db.get_connection()?;
        Ok(con
            .scan()?
            .collect::<Vec<String>>() // collect to keys (type info needed)
            .iter() // reiterate
            .map(|key| {
                let data: String = con.get(&*key)?;
                Ok(XPUser {
                    user_id: UserId::from(key.parse::<u64>().expect("Failed to get XPUser from User ID")), // should never fail
                    meta: serde_json::from_str(&*data)?,
                })
            }) // turn String into XPUser
            .collect::<Result<Vec<XPUser>, QueryError>>()?)
    }

    fn get_user(db: &redis::Client, id: UserId) -> Result<XPMeta, QueryError> {
        let con = db.get_connection()?;
        let data: String = con.get(&*(id.0.to_string()))?;
        Ok(serde_json::from_str(&*data)?)
    }

    fn add_user(db: &redis::Client, user: XPUser) -> Result<XPMeta, QueryError> {
        let con = db.get_connection()?;
        let id = user.user_id.0.to_string();
        con.set(&id, serde_json::to_string(&user.meta)?)?;
        let ins_text: String = con.get(&*id)?;
        let ins_obj = serde_json::from_str(&*ins_text)?;
        Ok(ins_obj)
    }

    fn add_xp(db: &redis::Client, id: UserId, meta: &XPMeta, xp: f64) -> Result<(), QueryError> {
        let con = db.get_connection()?;
        let new_xp_obj = XPMeta {
            xp: meta.xp + xp,
            last_activity: Utc::now(),
        };
        let obj = serde_json::to_string(&new_xp_obj)?;
        con.set(id.to_string(), obj)?;
        Ok(())
    }
}

impl EventHandler for Handler {
    fn message(&self, ctx: Context, new_message: Message) {
        if !new_message.is_own() {
            // check if user is in database
            // if user is in database, and the timeout is complete add xp
            //info!("{:?}", new_message);
            let lock = ctx.data.lock();
            let state: &State = lock.get::<State>().expect("Failed to get state");
            let db = &state.db;
            if let Ok(meta) = Handler::get_user(&db, new_message.author.id) {
                if Utc::now().signed_duration_since(meta.last_activity)
                    > chrono::Duration::seconds(5)
                {
                    let mut rng = thread_rng();
                    let xp = rng.gen_range(0.3, 0.5);
                    let res = Handler::add_xp(&db, new_message.author.id, &meta, xp).is_ok();
                    if res {
                        info!(
                            "Successfully added {} xp to {}",
                            xp, new_message.author.name
                        );
                        // check if this was a level up
                        let alpha = state
                            .ranks
                            .clone()
                            .into_iter()
                            .filter(|r| meta.xp + xp >= r.required_xp)
                            .collect::<HashSet<Rank>>();
                        let beta = state
                            .ranks
                            .clone()
                            .into_iter()
                            .filter(|r| meta.xp < r.required_xp)
                            .collect::<HashSet<Rank>>();
                        let mut intersect = alpha.intersection(&beta);
                        if let Some(rank) = intersect.next() {
                            let xp_usr = XPUser {
                                user_id: new_message.author.id,
                                meta: XPMeta {
                                    xp: meta.xp + xp,
                                    ..meta
                                },
                            };
                            let role = rank.role_id;
                            let cached = role.to_role_cached();
                            info!("{:?}", cached);
                            if let Some(a) = cached {
                                info!("{:?}", a);
                                info!("{:?}", a.find_guild());
                            }
                            if let Some(mut memb) = new_message.member() {
                                // remove all roles we are !!not!!
                                info!(
                                    "removing roles: {:?}",
                                    memb.remove_roles(
                                        state
                                            .ranks
                                            .clone()
                                            .into_iter()
                                            .filter(|r| r != rank)
                                            .map(|r| r.role_id)
                                            .collect::<Vec<serenity::model::id::RoleId>>()
                                            .as_slice()
                                    )
                                );
                                info!("adding role: {:?}", memb.add_role(role));
                                let embed = new_message.channel_id.send_message(|_| {
                                    create_level_up_embed(
                                        xp_usr,
                                        state.ranks.clone(),
                                        new_message.timestamp,
                                        new_message.author.avatar_url(),
                                    )
                                });
                                if let Ok(embed) = embed {
                                    thread::spawn(move || {
                                        thread::sleep(std::time::Duration::from_millis(15000));
                                        info!("{:?}", embed.delete());
                                    });
                                }
                            }
                        }
                    } else {
                        error!("Failed to add xp! {:?}", res);
                    }
                }
            } else {
                // if user is not in database, create them and attribute xp
                let new = XPUser {
                    user_id: new_message.author.id,
                    meta: XPMeta {
                        last_activity: Utc::now(),
                        xp: 0.0,
                    },
                };
                let res = Handler::add_user(&db, new);
                if res.is_ok() {
                    info!("Successfully added user {:?}", res.unwrap());
                } else {
                    error!("WTF! Couldn't add user {:?}", res);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct XPUser {
    user_id: UserId,
    meta: XPMeta,
}

impl XPUser {
    fn achieved(&self, ranks: &Vec<Rank>) -> Vec<Rank> {
        ranks
            .clone()
            .into_iter()
            .filter(|r| self.meta.xp >= r.required_xp)
            .collect::<Vec<Rank>>()
    }
    fn left(&self, ranks: &Vec<Rank>) -> Vec<Rank> {
        ranks
            .clone()
            .into_iter()
            .filter(|r| self.meta.xp < r.required_xp)
            .collect::<Vec<Rank>>()
    }
    fn level(&self, ranks: &[Rank]) -> Option<Rank> {
        let a = self.achieved(&ranks.to_vec());
        if a.is_empty() {
            None
        } else {
            Some(a[0].clone())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReifiedXPUser {
    xp_user: XPUser,
    username: String,
    discriminator: u16,
    level: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct XPMeta {
    xp: f64,
    last_activity: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct Rank {
    role_id: RoleId,
    required_xp: f64,
}

impl cmp::PartialEq for Rank {
    fn eq(&self, other: &Rank) -> bool {
        self.role_id == other.role_id
    }
}
impl cmp::Eq for Rank {}
impl hash::Hash for Rank {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.role_id.hash(state);
    }
}

enum ParseError {
    Float(num::ParseFloatError),
    Int(num::ParseIntError),
}

impl From<num::ParseFloatError> for ParseError {
    fn from(pfe: num::ParseFloatError) -> ParseError {
        ParseError::Float(pfe)
    }
}

impl From<num::ParseIntError> for ParseError {
    fn from(pfe: num::ParseIntError) -> ParseError {
        ParseError::Int(pfe)
    }
}

impl Rank {
    fn from(s: String) -> Result<Rank, ParseError> {
        let data: Vec<String> = s.split_whitespace().map(String::from).collect();
        Ok(Rank {
            role_id: RoleId::from(data[0].parse::<u64>()?),
            required_xp: data[1].parse::<f64>()?,
        })
    }
}

impl TypeMapKey for State {
    type Value = State;
}

fn setup_logger() -> Result<(), fern::InitError> {
    use colored::Colorize;
    fn colorize_format(level: log::Level) -> String {
        fn color_for_level(level: log::Level) -> colored::Color {
            match level {
                log::Level::Error => colored::Color::Red,
                log::Level::Warn => colored::Color::Yellow,
                log::Level::Info => colored::Color::Green,
                log::Level::Debug => colored::Color::Cyan,
                log::Level::Trace => colored::Color::Magenta,
            }
        }
        match level {
            log::Level::Error => "ERROR",
            log::Level::Warn => " WARN",
            log::Level::Info => " INFO",
            log::Level::Debug => "DEBUG",
            log::Level::Trace => "TRACE",
        }
        .color(color_for_level(level))
        .to_string()
    }

    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} {} {}: {}",
                colorize_format(record.level()),
                chrono::Local::now().format("%k:%M"),
                record.target(),
                message
            ))
        })
        .level(log::LevelFilter::Debug)
        .chain(std::io::stdout())
        .chain(fern::log_file("output.log")?)
        // suppress hyper, tokio & serenity
        .level_for("tokio_reactor", log::LevelFilter::Off)
        .level_for("hyper", log::LevelFilter::Info)
        .level_for("serenity", log::LevelFilter::Off)
        .apply()?;
    Ok(())
}

#[derive(Debug, Clone)]
struct State {
    ranks: Vec<Rank>,
    db: redis::Client,
}

fn main() -> Result<(), std::io::Error> {
    use std::io::Read;

    setup_logger().expect("Failed to setup logger");

    let config_file = "config.txt";

    // check if config file exists
    if !path::Path::new(config_file).exists() {
        fs::File::create(config_file)?;
    }

    // read config
    let mut buf = String::new();
    fs::File::open(config_file)?.read_to_string(&mut buf)?;
    let mut iter = buf.split('\n');
    if let Some(guild_str) = iter.next() {
        let guild = guild_str.parse::<u64>().expect("Failed to parse guild");
        let ranks: Vec<Rank> = iter
            .map(String::from)
            .filter(|s| !s.is_empty())
            .map(Rank::from)
            .filter_map(Result::ok)
            .collect();
        

        info!("Serving only guild {} ({} ranks)", guild, ranks.len());

        let redis_client =
            { redis::Client::open("redis://127.0.0.1").expect("Failed to connect to redis") };

        let state = State {
            ranks,
            db: redis_client,
        };

        let mut client = Client::new(&env::var("DISCORD_TOKEN").expect("token"), Handler)
            .expect("Error creating client");
        {
            let mut data = client.data.lock();
            data.insert::<State>(state);
        }

        client.with_framework(
            StandardFramework::new()
                .configure(|c| c.prefix("/"))
                .on("leaderboard", |ctx, msg, args| {
                    if let Some(chan) = msg.channel() {
                        let c = chan.id();
                        let lock = ctx.data.lock();
                        let state: &State = lock.get::<State>().expect("Failed to get State");
                        let result = Handler::get_users(&state.db);
                        if let Ok(users) = result {
                            info!("{:?}", args);
                            c.send_message(|_| {
                                create_leaderboard_embed(
                                    users,
                                    state.ranks.clone(),
                                    msg.timestamp,
                                    args.current()
                                        .map(|x| x.parse::<usize>().unwrap_or(5))
                                        .unwrap_or(5),
                                )
                            }).expect("Failed to send message");
                        } else {
                            msg.reply(&*format!("Could not grab users ```{:?}```", result))
                                .expect("Failed to send message");
                        }
                    } else {
                        msg.reply("You are not in a guild!")
                            .expect("Failed to send message");
                    }
                    Ok(())
                })
                .on("stats", |ctx, msg, mut args| {
                    fn parse(
                        args: &mut serenity::framework::standard::Args,
                        msg: &Message,
                    ) -> (UserId, bool, Option<String>) {
                        let arg = args.single::<UserId>();
                        if arg.is_ok() {
                            let user = arg.unwrap();
                            if let Ok(user_obj) = user.to_user() {
                                return (user, false, user_obj.avatar_url());
                            }
                        }
                        (msg.author.id, true, msg.author.avatar_url())
                    }

                    let (des_user, myself, avatar) = parse(&mut args, &msg);
                    let lock = ctx.data.lock();
                    let state: &State = lock.get::<State>().expect("Failed to get State");
                    if let Some(chan) = msg.channel() {
                        if let Ok(user) = Handler::get_user(&state.db, des_user) {
                            chan.id()
                                .send_message(|_| {
                                    create_info_embed(
                                        XPUser {
                                            user_id: des_user,
                                            meta: user,
                                        },
                                        &state.ranks,
                                        msg.timestamp,
                                        myself,
                                        avatar,
                                    )
                                })
                                .expect("Failed to send message");
                        } else {
                            msg.reply("Can't find that user")
                                .expect("Failed to send message");
                        }
                    }
                    Ok(())
                })
                .on("announce", |_ctx, msg, mut args| {
                    info!("{:?}", args);
                    let des_chan_opt = args.single::<ChannelId>();
                    let json_opt = args.single::<String>();
                    if let Some(guild) = msg.guild_id {
                        if let Ok(des_chan) = des_chan_opt {
                            let chans = guild.channels().expect("Failed to get channels");
                            let chan = chans.get(&des_chan);
                            if let Ok(json_link) = json_opt {
                                if let Ok(emb) = dbg!(announce::parse_announcement(&*json_link)) {
                                    if let Some(c) = chan {
                                        c.send_message(|_| emb.into_embed())
                                            .expect("Failed to send message");
                                    }
                                }
                            }
                        }
                    }
                    Ok(())
                }),
        );

        if let Err(why) = client.start() {
            error!("An error occurred while running the client: {:?}", why);
        }
    } else {
        error!("Invalid configuration!");
    }

    Ok(())
}

const BLAST_ICON_URL: &str =
    "https://cdn.discordapp.com/icons/506219319030448128/342d618bf1cb75d7ce71c44a0904b437.webp";

fn create_level_up_embed(
    user: XPUser,
    ranks: Vec<Rank>,
    at: DateTime<FixedOffset>,
    avatar_url: Option<String>,
) -> serenity::builder::CreateMessage {
    fn make_description(user_id: String, new_rank: &Rank, rem: f64, next: Option<&Rank>) -> String {
        if let Some(next_rank) = next {
            format!("Congratulations <@!{}>, you have just leveled up to rank <@&{}>. You need **{:.3}** more XP to achieve rank <@&{}>.", user_id, new_rank.role_id.to_string(), rem, next_rank.role_id.to_string())
        } else {
            format!("Congratulations <@!{}>, you are at the max level!", user_id)
        }
    }

    let ach: Vec<Rank> = ranks
        .clone()
        .into_iter()
        .filter(|r| user.meta.xp >= r.required_xp)
        .collect();
    let rem: Vec<Rank> = ranks
        .into_iter()
        .filter(|r| user.meta.xp < r.required_xp)
        .collect();

    warn!("achieved: {:?}", ach);
    warn!("remaining: {:?}", rem);

    serenity::builder::CreateMessage::default().embed(|mut e: serenity::builder::CreateEmbed| {
        e = e
            .author(|a| a.name("Blast — Level up!").icon_url(BLAST_ICON_URL))
            .description(&*make_description(
                user.user_id.0.to_string(),
                &ach[ach.len() - 1],
                rem.get(0)
                    .map(|r| r.required_xp - user.meta.xp)
                    .unwrap_or(0.0),
                rem.get(0),
            ))
            .timestamp(&at)
            .footer(|f| f.text(&*format!("You have {:.3} XP", user.meta.xp)));
        if let Some(a_url) = avatar_url {
            e = e.thumbnail(&*a_url);
        }
        e
    })
}

fn create_info_embed(
    xp_user: XPUser,
    ranks: &Vec<Rank>,
    at: DateTime<FixedOffset>,
    myself: bool,
    avatar: Option<String>,
) -> serenity::builder::CreateMessage {
    if let Some(next) = xp_user.left(&ranks).get(0) {
        if let Some(current) = xp_user.level(&ranks) {
            let left_xp = next.required_xp - xp_user.meta.xp;
            serenity::builder::CreateMessage::default().embed(|mut e| { 
              e = e.author(|a| a.name("Blast — Statistics").icon_url(BLAST_ICON_URL)).description(format!("{} at rank <@&{}> with **{:.3}** XP. {} need **{:.3}** more XP to advance to rank <@&{}>.", {
				if myself {
					"You're currently".to_string()
				} else {
					format!("<@!{}> is", xp_user.user_id.0.to_string())
				}
			}, current.role_id.0.to_string(), xp_user.meta.xp, if myself {
					"You"
				} else {
					"They"
				}, left_xp, next.role_id.0.to_string())).timestamp(&at); 
		if let Some(avatar_url) = avatar {
			e = e.thumbnail(avatar_url);
		}
		e
		})
        } else {
            let left_xp = next.required_xp - xp_user.meta.xp;
            serenity::builder::CreateMessage::default().embed(|mut e: serenity::builder::CreateEmbed| { e = e.author(|a| a.name("Blast — Statistics").icon_url(BLAST_ICON_URL))
		.description(format!("{} no rank and **{:.3}** XP. {} need **{:.3}** more XP to advance to rank <@&{}>.", {
				if myself {
					"You currently have".to_string()
				} else {
					format!("<@!{}> has", xp_user.user_id.0.to_string())
				}
			}, xp_user.meta.xp,left_xp, {
				if myself {
					"You"
				} else {
					"They"
				}
			}, next.role_id.0.to_string())).timestamp(&at); 
		if let Some(avatar_url) = avatar {
			e = e.thumbnail(avatar_url);
		}
		e})
        }
    } else {
        serenity::builder::CreateMessage::default().embed(
            |mut e: serenity::builder::CreateEmbed| {
                e = e
                    .author(|a| a.name("Blast — Statistics").icon_url(BLAST_ICON_URL))
                    .description(format!(
                        "{} at the max rank <@&{}>, with **{:.3}** XP.",
                        {
                            if myself {
                                "You're currently ".to_string()
                            } else {
                                format!("<@!{}> is", xp_user.user_id.0.to_string())
                            }
                        },
                        xp_user
                            .achieved(&ranks)
                            .last()
                            .unwrap()
                            .role_id
                            .0
                            .to_string(),
                        xp_user.meta.xp
                    ))
                    .timestamp(&at);
                if let Some(avatar_url) = avatar {
                    e = e.thumbnail(avatar_url);
                }
                e
            },
        )
    }
}

/// create_leaderboard_embed assumes users is already sorted
fn create_leaderboard_embed(
    users: Vec<XPUser>,
    ranks: Vec<Rank>,
    at: DateTime<FixedOffset>,
    cap: usize,
) -> serenity::builder::CreateMessage {
    fn reify_user(xp_user: &XPUser, ranks: Vec<Rank>) -> Result<ReifiedXPUser, serenity::Error> {
        let user_id = xp_user.user_id;
        let xp = xp_user.meta.xp;
        let user = user_id.to_user()?;
        Ok(ReifiedXPUser {
            xp_user: xp_user.clone(),
            username: user.name,
            discriminator: user.discriminator,
            level: {
                let opt = ranks
                    .iter()
                    .enumerate()
                    .filter(|&x| xp >= x.1.required_xp)
                    .last();
                if opt.is_some() {
                    opt.unwrap().0 + 1
                } else {
                    0
                }
            },
        })
    }
    fn xpuser_to_str(dat: (usize, ReifiedXPUser)) -> String {
        fn get_emoji(ind: usize) -> String {
            match ind {
                1 => "🥇",
                2 => "🥈",
                3 => "🥉",
                _ => "▫",
            }
            .to_string()
        }
        let (ind, usr) = dat;
        format!(
            "{} - #{}. <@!{}> (level **{}**, **{:.3}** XP)",
            get_emoji(ind),
            ind,
            usr.xp_user.user_id.0,
            usr.level,
            usr.xp_user.meta.xp
        )
    }
    serenity::builder::CreateMessage::default().embed(|e: serenity::builder::CreateEmbed| {
        e.author(|a| a.name("Blast — Leaderboard").icon_url(BLAST_ICON_URL))
            .description({
                let mut sorted = users.clone();
                sorted.sort_by(|a, b| {
                    let bxp = b.meta.xp;
                    let axp = a.meta.xp;
                    axp.partial_cmp(&bxp).unwrap_or(std::cmp::Ordering::Equal)
                });
                let user_strs: Vec<String> = sorted
                    .iter()
                    .rev()
                    .take(if cap < 1 { 5 } else { cap })
                    .map(|x| reify_user(x, ranks.clone())) // TODO: Maybe wanna rewrite to avoid clone
                    .filter_map(Result::ok)
                    .enumerate()
                    .map(|x| (x.0 + 1, x.1)) // move level up by one for display
                    .map(xpuser_to_str)
                    .collect();
                user_strs.join("\n")
            })
            .timestamp(&at)
    })
}
