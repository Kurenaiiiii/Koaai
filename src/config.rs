pub const C_MAIN: u32 = 0x2B2D31;
pub const C_SUCCESS: u32 = 0x23A55A;
pub const C_ERROR: u32 = 0xF23F42;
pub const C_WARN: u32 = 0xF0B132;
pub const C_QUEUE: u32 = 0x5865F2;
pub const C_NP: u32 = 0x1DB954;
pub const C_INFO: u32 = 0x4A90D9;

pub mod emojis {
    pub const SUCCESS: &str = "<:9294passed:1500104823675162645>";
    pub const ERROR: &str = "<:3595failed:1500104734365712536>";
    pub const WARN: &str = "<:6371win11warningicon:1500104790016000040>";
    pub const LOADING: &str = "<:4598unstableping:1500104751059046481>";
    pub const PING: &str = "<:5801stableping:1500104781367349269>";
    pub const ONLINE: &str = "<:5251onlinestatus:1500104769371639938>";
    pub const OFFLINE: &str = "<:5163dndstatus:1500104767005921462>";
    pub const WAVE: &str = "<a:524346sniffdog:1500104999844319272>";
    pub const WRENCH: &str = "<:381258twotonedstaffids:1500106729441202197>";
    pub const ROCKET: &str = "<:798008booster1:1500107215107788801>";
    pub const SYNC: &str = "<:72381sync:1500107718223069308>";

    pub const CAT_MUSIC: &str = "<:174343heartmusicnote:1500108127238885406>";
    pub const CAT_PLAYLIST: &str = "<:22838playlist:1500108125070692352>";
    pub const CAT_AUDIO: &str = "<:4767voiceevent:1500104757568868475>";
    pub const CAT_SETTINGS: &str = "<:2636securityfilter:1500104728896606381>";
    pub const CAT_INFO: &str = "<:908915information:1500080758239531159>";

    pub const PLAY: &str = "<:circleplay:1500101049183113266>";
    pub const PAUSE: &str = "<:pause:1500101615951151207>";
    pub const SKIP: &str = "<:fastforward:1500109903585345678>";
    pub const BACK: &str = "<:rewind:1500110006132015205>";
    pub const STOP: &str = "<:ban:1500101892385149059>";
    pub const LOOP: &str = "<:repeat1:1500101050915491891>";
    pub const SHUFFLE: &str = "<:shuffle:1500109370506084512>";
    pub const CLEAR: &str = "<:trash2:1500109445298917426>";
    pub const FORWARD: &str = "<:fastforward:1500109903585345678>";
    pub const REWIND: &str = "<:rewind:1500110006132015205>";
    pub const VOLUME: &str = "<:volume2:1500110144074420406>";
    pub const FOLDER: &str = "<:folder:1500110368289063073>";
    pub const SLEEP: &str = "<:bed:1500110477731041330>";

    pub const YOUTUBE: &str = "<:54079youtube:1500080671341674516>";
    pub const SPOTIFY: &str = "<:2320spotify:1500080653188989020>";
    pub const SOUNDCLOUD: &str = "<:32976soundcloud:1500115990359576688>";
    pub const APPLEMUSIC: &str = "<:3265applemusic:1500116142478594090>";
}

pub fn owner_id() -> String {
    std::env::var("OWNER_ID").unwrap_or_else(|_| "778163158312550410".into())
}
