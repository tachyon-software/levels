/// used as a facade for creating & parsing announcements

#[derive(Deserialize, Debug, Clone)]
struct Author {
    name: String,
    icon_url: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct Footer {
    icon_url: Option<String>,
    text: String,
}

impl Into<serenity::builder::CreateEmbedFooter> for Footer {
    fn into(self) -> serenity::builder::CreateEmbedFooter {
        let mut d = serenity::builder::CreateEmbedFooter::default().text(&*self.text);
        if let Some(ic_url) = self.icon_url {
            d = d.icon_url(&*ic_url);
        }
        d
    }
}

impl Into<serenity::builder::CreateEmbedAuthor> for Author {
    fn into(self) -> serenity::builder::CreateEmbedAuthor {
        let mut d = serenity::builder::CreateEmbedAuthor::default().name(&*self.name);
        if let Some(icon_url) = self.icon_url {
            d = d.icon_url(&*icon_url)
        }
        if let Some(url) = self.url {
            d = d.url(&*url)
        }
        d
    }
}

#[derive(Deserialize, Debug, Clone)]
struct OneShot {
    url: String,
}

#[derive(Deserialize, Debug, Clone)]
struct Field {
    // for deserialization purpsoes
    name: String,
    value: String,
    inline: Option<bool>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ParsedAnnouncement {
    description: Option<String>,
    color: Option<i32>,
    timestamp: Option<chrono::DateTime<chrono::Utc>>,
    author: Option<Author>,
    fields: Option<Vec<Field>>,
    footer: Option<Footer>,
    image: Option<OneShot>,
    thumbnail: Option<OneShot>,
    title: Option<String>,
}

impl ParsedAnnouncement {
    pub fn into_embed(self) -> serenity::builder::CreateMessage {
        serenity::builder::CreateMessage::default().embed(
            |mut e: serenity::builder::CreateEmbed| {
                if let Some(author) = self.author {
                    e = e.author(|_| author.into());
                }

                if let Some(footer) = self.footer {
                    e = e.footer(|_| footer.into());
                }

                if let Some(desc) = self.description {
                    e = e.description(desc);
                }

                if let Some(timestamp) = self.timestamp {
                    e = e.timestamp(&timestamp);
                }

                if let Some(color) = self.color {
                    e = e.color(color);
                }

                if let Some(title) = self.title {
                    e = e.title(title);
                }

                if let Some(fields) = self.fields {
                    for field in fields {
                        e = e.field(field.name, field.value, field.inline.unwrap_or(false))
                    }
                }

                if let Some(thumbnail) = self.thumbnail {
                    e = e.thumbnail(thumbnail.url);
                }

                if let Some(image) = self.image {
                    e = e.image(image.url);
                }

                e
            },
        )
    }
}

pub fn parse_announcement(link: &str) -> Result<ParsedAnnouncement, Box<std::error::Error>> {
    Ok(reqwest::get(link)?.json()?)
}
