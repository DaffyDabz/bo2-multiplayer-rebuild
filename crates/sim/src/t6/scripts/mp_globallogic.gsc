// bo2mp: maps/mp/gametypes/_globallogic for Black Ops II multiplayer.
//
// No zone on a PC install carries this script, yet every game type's main,
// the spawn, score and player scripts call into it. This is OUR CODE, written
// from what those scripts call and wait for: the level and game[] fields they
// read, the callbacks they expect in level.*, the notifies they wait on
// ("prematch_over", "grace_period_ending", "game_ended", "round_end_done"),
// and the end of a match through the game's own outcome screens, final
// killcam and next round. Compiled by gsc_t6::compile at map load.
//
// A local match: no league, no wagers, no online services; ranked only in
// Combat Training (gamemodeisusingxp, lane C).

init()
{
    if ( !isdefined( level.tweakablesinitialized ) )
        maps/mp/gametypes/_tweakables::init();
    level.splitscreen = 0;
    level.xenon = 0;
    level.ps3 = 0;
    level.wiiu = 0;
    level.console = 0;
    level.onlinegame = 0;
    level.systemlink = 0;
    level.rankedmatch = gamemodeisusingxp();
    level.leaguematch = 0;
    level.wagermatch = 0;
    level.contractsenabled = 0;
    level.script = tolower( getdvar( "mapname" ) );
    level.gametype = tolower( getdvar( "g_gametype" ) );
    level.teambased = 0;
    level.teamcount = getgametypesetting( "teamCount" );
    level.multiteam = level.teamcount > 2;
    level.teams = [];
    level.teamindex = [];
    level.teams["allies"] = "allies";
    level.teams["axis"] = "axis";
    level.teamindex["neutral"] = 0;
    level.teamindex["allies"] = 1;
    level.teamindex["axis"] = 2;
    for ( index = 3; index <= level.teamcount; index++ )
    {
        level.teams["team" + index] = "team" + index;
        level.teamindex["team" + index] = index;
    }
    level.overrideteamscore = 0;
    level.overrideplayerscore = 0;
    level.displayhalftimetext = 0;
    level.displayroundendtext = 1;
    level.endgameonscorelimit = 1;
    level.endgameontimelimit = 1;
    level.scoreroundbased = 0;
    level.resetplayerscoreeveryround = 0;
    level.gameforfeited = 0;
    level.forceautoassign = 0;
    level.halftimetype = "halftime";
    level.halftimesubcaption = &"MP_SWITCHING_SIDES_CAPS";
    level.laststatustime = 0;
    level.lastslowprocessframe = 0;
    level.waswinning = [];
    level.placement = [];
    foreach ( team in level.teams )
        level.placement[team] = [];
    level.placement["all"] = [];
    level.postroundtime = 7.0;
    level.inovertime = 0;
    level.defaultoffenseradius = 560;
    level.dropteam = getdvarint( "sv_maxclients" );
    level.infinalkillcam = 0;
    maps/mp/gametypes/_globallogic_ui::init();
    registerdvars();
    maps/mp/gametypes/_class::initperkdvars();
    level.oldschool = getdvarint( "scr_oldschool" ) == 1;
    precachemodel( "tag_origin" );
    precacherumble( "dtp_rumble" );
    precacherumble( "slide_rumble" );
    precachestatusicon( "hud_status_dead" );
    precachestatusicon( "hud_status_connecting" );
    maps/mp/_burnplayer::initburnplayer();
    if ( !isdefined( game["tiebreaker"] ) )
        game["tiebreaker"] = 0;
    maps/mp/gametypes/_globallogic_audio::registerdialoggroup( "introboost", 1 );
    maps/mp/gametypes/_globallogic_audio::registerdialoggroup( "status", 1 );
    level.disablechallenges = 1;
    level.disablestattracking = 1;
}

// The dvars the scripts read and nothing else sets: a fresh match's.
registerdvars()
{
    if ( getdvar( "scr_oldschool" ) == "" )
        setdvar( "scr_oldschool", "0" );
    if ( getdvar( "ui_guncycle" ) == "" )
        setdvar( "ui_guncycle", "0" );
    if ( getdvar( "ui_weapon_tiers" ) == "" )
        setdvar( "ui_weapon_tiers", "0" );
    if ( getdvar( "scr_max_rank" ) == "" )
        setdvar( "scr_max_rank", "0" );
    if ( getdvar( "scr_min_prestige" ) == "" )
        setdvar( "scr_min_prestige", "0" );
}

blank( arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9, arg10 )
{
}

// Every level.* hook the game type may replace: spawning, scoring, the end
// of a match and its outcome screens, all pointing at the game's own scripts.
setupcallbacks()
{
    level.spawnplayer = maps/mp/gametypes/_globallogic_spawn::spawnplayer;
    level.spawnplayerprediction = maps/mp/gametypes/_globallogic_spawn::spawnplayerprediction;
    level.spawnclient = maps/mp/gametypes/_globallogic_spawn::spawnclient;
    level.spawnspectator = maps/mp/gametypes/_globallogic_spawn::spawnspectator;
    level.spawnintermission = maps/mp/gametypes/_globallogic_spawn::spawnintermission;
    level.onplayerscore = maps/mp/gametypes/_globallogic_score::default_onplayerscore;
    level.onteamscore = maps/mp/gametypes/_globallogic_score::default_onteamscore;
    level.wavespawntimer = ::wavespawntimer;
    level.spawnmessage = maps/mp/gametypes/_globallogic_spawn::default_spawnmessage;
    level.onspawnplayer = ::blank;
    level.onspawnplayerunified = ::blank;
    level.onspawnspectator = maps/mp/gametypes/_globallogic_defaults::default_onspawnspectator;
    level.onspawnintermission = maps/mp/gametypes/_globallogic_defaults::default_onspawnintermission;
    level.onrespawndelay = ::blank;
    level.onforfeit = maps/mp/gametypes/_globallogic_defaults::default_onforfeit;
    level.ontimelimit = maps/mp/gametypes/_globallogic_defaults::default_ontimelimit;
    level.onscorelimit = maps/mp/gametypes/_globallogic_defaults::default_onscorelimit;
    level.onalivecountchange = maps/mp/gametypes/_globallogic_defaults::default_onalivecountchange;
    level.ondeadevent = undefined;
    level.ononeleftevent = maps/mp/gametypes/_globallogic_defaults::default_ononeleftevent;
    level.giveteamscore = maps/mp/gametypes/_globallogic_score::giveteamscore;
    level.onlastteamaliveevent = maps/mp/gametypes/_globallogic_defaults::default_onlastteamaliveevent;
    level.gettimelimit = maps/mp/gametypes/_globallogic_defaults::default_gettimelimit;
    level.getteamkillpenalty = maps/mp/gametypes/_globallogic_defaults::default_getteamkillpenalty;
    level.getteamkillscore = maps/mp/gametypes/_globallogic_defaults::default_getteamkillscore;
    level.iskillboosting = maps/mp/gametypes/_globallogic_score::default_iskillboosting;
    level._setteamscore = maps/mp/gametypes/_globallogic_score::_setteamscore;
    level._setplayerscore = maps/mp/gametypes/_globallogic_score::_setplayerscore;
    level._getteamscore = maps/mp/gametypes/_globallogic_score::_getteamscore;
    level._getplayerscore = maps/mp/gametypes/_globallogic_score::_getplayerscore;
    level.onprecachegametype = ::blank;
    level.onstartgametype = ::blank;
    level.onplayerconnect = ::blank;
    level.onplayerdisconnect = ::blank;
    level.onplayerdamage = ::blank;
    level.onplayerkilled = ::blank;
    level.onplayerkilledextraunthreadedcbs = [];
    level.onteamoutcomenotify = maps/mp/gametypes/_hud_message::teamoutcomenotify;
    level.onoutcomenotify = maps/mp/gametypes/_hud_message::outcomenotify;
    level.onteamwageroutcomenotify = maps/mp/gametypes/_hud_message::teamwageroutcomenotify;
    level.onwageroutcomenotify = maps/mp/gametypes/_hud_message::wageroutcomenotify;
    level.setmatchscorehudelemforteam = maps/mp/gametypes/_hud_message::setmatchscorehudelemforteam;
    level.onendgame = ::blank;
    level.onroundendgame = maps/mp/gametypes/_globallogic_defaults::default_onroundendgame;
    level.onmedalawarded = ::blank;
    maps/mp/gametypes/_globallogic_ui::setupcallbacks();
}

// ---- the start of a match ---------------------------------------------------

callback_startgametype()
{
    level.prematchperiod = 0;
    level.intermission = 0;
    setmatchflag( "cg_drawSpectatorMessages", 1 );
    setmatchflag( "game_ended", 0 );
    if ( !isdefined( game["gamestarted"] ) )
    {
        // The teams' factions (the map's teamset script sets its own first).
        if ( !isdefined( game["allies"] ) )
            game["allies"] = "seals";
        if ( !isdefined( game["axis"] ) )
            game["axis"] = "pmc";
        if ( !isdefined( game["attackers"] ) )
            game["attackers"] = "allies";
        if ( !isdefined( game["defenders"] ) )
            game["defenders"] = "axis";
        foreach ( team in level.teams )
        {
            if ( !isdefined( game[team] ) )
                game[team] = "pmc";
        }
        if ( !isdefined( game["state"] ) )
            game["state"] = "playing";
        precacherumble( "damage_heavy" );
        precacherumble( "damage_light" );
        precacheshader( "white" );
        precacheshader( "black" );
        game["strings"]["press_to_spawn"] = &"PLATFORM_PRESS_TO_SPAWN";
        if ( level.teambased )
            game["strings"]["waiting_for_teams"] = &"MP_WAITING_FOR_TEAMS";
        else
            game["strings"]["waiting_for_teams"] = &"MP_WAITING_FOR_PLAYERS";
        game["strings"]["opponent_forfeiting_in"] = &"MP_OPPONENT_FORFEITING_IN";
        game["strings"]["match_starting_in"] = &"MP_MATCH_STARTING_IN";
        game["strings"]["spawn_next_round"] = &"MP_SPAWN_NEXT_ROUND";
        game["strings"]["waiting_to_spawn"] = &"MP_WAITING_TO_SPAWN";
        game["strings"]["waiting_to_spawn_ss"] = &"MP_WAITING_TO_SPAWN_SS";
        game["strings"]["you_will_spawn"] = &"MP_YOU_WILL_RESPAWN";
        game["strings"]["match_starting"] = &"MP_MATCH_STARTING";
        game["strings"]["change_class"] = &"MP_CHANGE_CLASS_NEXT_SPAWN";
        game["strings"]["last_stand"] = &"MPUI_LAST_STAND";
        game["strings"]["cowards_way"] = &"PLATFORM_COWARDS_WAY_OUT";
        game["strings"]["tie"] = &"MP_MATCH_TIE";
        game["strings"]["round_draw"] = &"MP_ROUND_DRAW";
        game["strings"]["enemies_eliminated"] = &"MP_ENEMIES_ELIMINATED";
        game["strings"]["score_limit_reached"] = &"MP_SCORE_LIMIT_REACHED";
        game["strings"]["round_limit_reached"] = &"MP_ROUND_LIMIT_REACHED";
        game["strings"]["time_limit_reached"] = &"MP_TIME_LIMIT_REACHED";
        game["strings"]["players_forfeited"] = &"MP_PLAYERS_FORFEITED";
        game["strings"]["other_teams_forfeited"] = &"MP_OTHER_TEAMS_FORFEITED";
        [[ level.onprecachegametype ]]();
        game["gamestarted"] = 1;
        game["totalKills"] = 0;
        foreach ( team in level.teams )
        {
            game["teamScores"][team] = 0;
            game["totalKillsTeam"][team] = 0;
        }
        level.prematchperiod = getgametypesetting( "prematchperiod" );
    }
    else
        level.prematchperiod = getgametypesetting( "preroundperiod" );
    if ( !isdefined( game["timepassed"] ) )
        game["timepassed"] = 0;
    if ( !isdefined( game["roundsplayed"] ) )
        game["roundsplayed"] = 0;
    setroundsplayed( game["roundsplayed"] );
    if ( isdefined( game["overtime_round"] ) )
        setmatchflag( "overtime", 1 );
    else
        setmatchflag( "overtime", 0 );
    if ( !isdefined( game["roundwinner"] ) )
        game["roundwinner"] = [];
    if ( !isdefined( game["lastroundscore"] ) )
        game["lastroundscore"] = [];
    if ( !isdefined( game["roundswon"] ) )
        game["roundswon"] = [];
    if ( !isdefined( game["roundswon"]["tie"] ) )
        game["roundswon"]["tie"] = 0;
    foreach ( team in level.teams )
    {
        if ( !isdefined( game["roundswon"][team] ) )
            game["roundswon"][team] = 0;
        level.teamspawnpoints[team] = [];
        level.spawn_point_team_class_names[team] = [];
    }
    level.skipvote = 0;
    level.gameended = 0;
    setdvar( "g_gameEnded", 0 );
    // default_mp_gamesettings.cfg: "set scr_showperksonspawn 1"
    if ( getdvar( "scr_showperksonspawn" ) == "" )
        setdvar( "scr_showperksonspawn", 1 );
    level.objidstart = 0;
    level.forcedend = 0;
    level.hostforcedend = 0;
    level.hardcoremode = getgametypesetting( "hardcoreMode" );
    if ( level.hardcoremode && !isdefined( level.friendlyfiredelaytime ) )
        level.friendlyfiredelaytime = 0;
    level.rankcap = getdvarint( "scr_max_rank" );
    level.minprestige = getdvarint( "scr_min_prestige" );
    level.usestartspawns = 1;
    // The game type's rules, from its settings.
    level.roundscorecarry = getgametypesetting( "roundscorecarry" );
    level.allowhitmarkers = getgametypesetting( "allowhitmarkers" );
    level.playerqueuedrespawn = getgametypesetting( "playerQueuedRespawn" );
    level.playerforcerespawn = getgametypesetting( "playerForceRespawn" );
    level.roundstartexplosivedelay = getgametypesetting( "roundStartExplosiveDelay" );
    level.roundstartkillstreakdelay = getgametypesetting( "roundStartKillstreakDelay" );
    level.perksenabled = getgametypesetting( "perksEnabled" );
    level.disableattachments = getgametypesetting( "disableAttachments" );
    level.disabletacinsert = getgametypesetting( "disableTacInsert" );
    level.disablecac = getgametypesetting( "disableCAC" );
    level.disableclassselection = getgametypesetting( "disableClassSelection" );
    level.disableweapondrop = getgametypesetting( "disableweapondrop" );
    level.onlyheadshots = getgametypesetting( "onlyHeadshots" );
    level.minimumallowedteamkills = getgametypesetting( "teamKillPunishCount" ) - 1;
    level.teamkillreducedpenalty = getgametypesetting( "teamKillReducedPenalty" );
    level.teamkillpointloss = getgametypesetting( "teamKillPointLoss" );
    level.teamkillspawndelay = getgametypesetting( "teamKillSpawnDelay" );
    level.deathpointloss = getgametypesetting( "deathPointLoss" );
    level.leaderbonus = getgametypesetting( "leaderBonus" );
    level.forceradar = getgametypesetting( "forceRadar" );
    level.playersprinttime = getgametypesetting( "playerSprintTime" );
    level.bulletdamagescalar = getgametypesetting( "bulletDamageScalar" );
    level.playermaxhealth = getgametypesetting( "playerMaxHealth" );
    level.playerhealthregentime = getgametypesetting( "playerHealthRegenTime" );
    level.playerrespawndelay = getgametypesetting( "playerRespawnDelay" );
    level.playerobjectiveheldrespawndelay = getgametypesetting( "playerObjectiveHeldRespawnDelay" );
    level.waverespawndelay = getgametypesetting( "waveRespawnDelay" );
    level.suicidespawndelay = getgametypesetting( "spawnsuicidepenalty" );
    level.teamkilledspawndelay = getgametypesetting( "spawnteamkilledpenalty" );
    level.maxsuicidesbeforekick = getgametypesetting( "maxsuicidesbeforekick" );
    level.spectatetype = getgametypesetting( "spectateType" );
    level.voip = spawnstruct();
    level.voip.deadchatwithdead = getgametypesetting( "voipDeadChatWithDead" );
    level.voip.deadchatwithteam = getgametypesetting( "voipDeadChatWithTeam" );
    level.voip.deadhearallliving = getgametypesetting( "voipDeadHearAllLiving" );
    level.voip.deadhearteamliving = getgametypesetting( "voipDeadHearTeamLiving" );
    level.voip.everyonehearseveryone = getgametypesetting( "voipEveryoneHearsEveryone" );
    level.voip.deadhearkiller = getgametypesetting( "voipDeadHearKiller" );
    level.voip.killershearvictim = getgametypesetting( "voipKillersHearVictim" );
    // The game's helpers, as a multiplayer match starts them.
    thread maps/mp/gametypes/_persistence::init();
    thread maps/mp/gametypes/_menus::init();
    thread maps/mp/gametypes/_hud::init();
    thread maps/mp/gametypes/_serversettings::init();
    thread maps/mp/gametypes/_clientids::init();
    thread maps/mp/teams/_teams::init();
    thread maps/mp/gametypes/_weapons::init();
    thread maps/mp/gametypes/_scoreboard::init();
    thread maps/mp/gametypes/_killcam::init();
    thread maps/mp/gametypes/_shellshock::init();
    thread maps/mp/gametypes/_deathicons::init();
    thread maps/mp/gametypes/_damagefeedback::init();
    thread maps/mp/gametypes/_healthoverlay::init();
    thread maps/mp/gametypes/_spectating::init();
    thread maps/mp/gametypes/_objpoints::init();
    thread maps/mp/gametypes/_gameobjects::init();
    thread maps/mp/gametypes/_spawnlogic::init();
    thread maps/mp/gametypes/_battlechatter_mp::init();
    thread maps/mp/killstreaks/_killstreaks::init();
    thread maps/mp/gametypes/_globallogic_audio::init();
    thread maps/mp/gametypes/_wager::init();
    thread maps/mp/bots/_bot::init();
    if ( level.rankedmatch )
        thread combattrainingbots();
    thread maps/mp/_decoy::init();
    thread maps/mp/_bb::init();
    if ( level.teambased )
        thread maps/mp/gametypes/_friendicons::init();
    thread maps/mp/gametypes/_hud_message::init();
    thread maps/mp/_multi_extracam::init();
    stringnames = getarraykeys( game["strings"] );
    for ( index = 0; index < stringnames.size; index++ )
        precachestring( game["strings"][stringnames[index]] );
    foreach ( team in level.teams )
        initteamvariables( team );
    level.maxplayercount = 0;
    level.activeplayers = [];
    level.allowannouncer = getgametypesetting( "allowAnnouncer" );
    if ( !isdefined( level.timelimit ) )
        maps/mp/_utility::registertimelimit( 1, 1440 );
    if ( !isdefined( level.scorelimit ) )
        maps/mp/_utility::registerscorelimit( 1, 500 );
    if ( !isdefined( level.roundlimit ) )
        maps/mp/_utility::registerroundlimit( 0, 10 );
    if ( !isdefined( level.roundwinlimit ) )
        maps/mp/_utility::registerroundwinlimit( 0, 10 );
    maps/mp/gametypes/_globallogic_utils::registerpostroundevent( ::postroundfinalkillcam );
    maps/mp/gametypes/_globallogic_utils::registerpostroundevent( maps/mp/gametypes/_wager::postroundsidebet );
    makedvarserverinfo( "ui_scorelimit" );
    makedvarserverinfo( "ui_timelimit" );
    makedvarserverinfo( "ui_allow_classchange", getdvar( "ui_allow_classchange" ) );
    if ( level.waverespawndelay )
    {
        foreach ( team in level.teams )
        {
            level.wavedelay[team] = level.waverespawndelay;
            level.lastwave[team] = 0;
        }
        level thread [[ level.wavespawntimer ]]();
    }
    level.inprematchperiod = 1;
    // A countdown of a few seconds either way, as the game does.
    if ( level.prematchperiod > 2.0 )
        level.prematchperiod = level.prematchperiod + ( randomfloat( 4 ) - 2 );
    if ( level.numlives || anyteamhaswavedelay() || level.playerqueuedrespawn )
        level.graceperiod = 15;
    else
        level.graceperiod = 5;
    level.ingraceperiod = 1;
    level.roundenddelay = 5;
    level.halftimeroundenddelay = 3;
    maps/mp/gametypes/_globallogic_score::updateallteamscores();
    level.killstreaksenabled = 1;
    if ( getdvar( "scr_game_rankenabled" ) == "" )
        setdvar( "scr_game_rankenabled", 1 );
    level.rankenabled = getdvarint( "scr_game_rankenabled" );
    if ( getdvar( "scr_game_medalsenabled" ) == "" )
        setdvar( "scr_game_medalsenabled", 1 );
    level.medalsenabled = getdvarint( "scr_game_medalsenabled" );
    level.friendlyfiredelay = getdvarint( "scr_game_friendlyFireDelay" );
    [[ level.onstartgametype ]]();
    level thread maps/mp/gametypes/_killcam::dofinalkillcam();
    thread startgame();
    level thread updategametypedvars();
}

startgame()
{
    thread maps/mp/gametypes/_globallogic_utils::gametimer();
    level.timerstopped = 0;
    prematchperiod();
    level notify( "prematch_over" );
    thread timelimitclock();
    thread graceperiod();
    thread watchmatchendingsoon();
    thread maps/mp/gametypes/_globallogic_audio::musiccontroller();
}

waitforplayers()
{
    starttime = gettime();
    while ( getnumconnectedplayers() < 1 )
    {
        wait 0.05;
        if ( gettime() - starttime > 120000 )
            exitlevel( 0 );
    }
}

// Frozen, weapons down, the countdown on screen; then everyone may move.
prematchperiod()
{
    setmatchflag( "hud_hardcore", level.hardcoremode );
    level endon( "game_ended" );
    if ( level.prematchperiod > 0 )
    {
        thread matchstarttimer();
        waitforplayers();
        wait level.prematchperiod;
    }
    else
    {
        matchstarttimerskip();
        wait 0.05;
    }
    level.inprematchperiod = 0;
    for ( index = 0; index < level.players.size; index++ )
    {
        level.players[index] maps/mp/_utility::freeze_player_controls( 0 );
        level.players[index] enableweapons();
    }
    maps/mp/gametypes/_wager::prematchperiod();
}

// "Match begins in" and the seconds counting down, a tick each second.
matchstarttimer()
{
    visionsetnaked( "mpIntro", 0 );
    text = maps/mp/gametypes/_hud_util::createserverfontstring( "objective", 1.5 );
    text maps/mp/gametypes/_hud_util::setpoint( "CENTER", "CENTER", 0, -40 );
    text.sort = 1001;
    text settext( game["strings"]["waiting_for_teams"] );
    text.foreground = 0;
    text.hidewheninmenu = 1;
    waitforplayers();
    text settext( game["strings"]["match_starting_in"] );
    timer = maps/mp/gametypes/_hud_util::createserverfontstring( "big", 2.2 );
    timer maps/mp/gametypes/_hud_util::setpoint( "CENTER", "CENTER", 0, 0 );
    timer.sort = 1001;
    timer.color = ( 1, 1, 0 );
    timer.foreground = 0;
    timer.hidewheninmenu = 1;
    timer maps/mp/gametypes/_hud::fontpulseinit();
    left = int( level.prematchperiod );
    if ( left >= 2 )
    {
        while ( left > 0 && !level.gameended )
        {
            timer setvalue( left );
            timer thread maps/mp/gametypes/_hud::fontpulse( level );
            if ( left == 2 )
                visionsetnaked( getdvar( "mapname" ), 3.0 );
            left--;
            foreach ( player in level.players )
                player playlocalsound( "uin_start_count_down" );
            wait 1.0;
        }
    }
    else
        visionsetnaked( getdvar( "mapname" ), 1.0 );
    timer maps/mp/gametypes/_hud_util::destroyelem();
    text maps/mp/gametypes/_hud_util::destroyelem();
}

matchstarttimerskip()
{
    visionsetnaked( getdvar( "mapname" ), 0 );
}

graceperiod()
{
    level endon( "game_ended" );
    if ( isdefined( level.graceperiodfunc ) )
        [[ level.graceperiodfunc ]]();
    else
        wait level.graceperiod;
    level notify( "grace_period_ending" );
    wait 0.05;
    level.ingraceperiod = 0;
    if ( game["state"] != "playing" )
        return;
    if ( level.numlives )
    {
        foreach ( player in level.players )
        {
            if ( !player.hasspawned && player.sessionteam != "spectator" && !isalive( player ) )
                player.statusicon = "hud_status_dead";
        }
    }
    level thread updateteamstatus();
}

watchmatchendingsoon()
{
    setdvar( "xblive_matchEndingSoon", 0 );
    level waittill( "match_ending_soon", reason );
    setdvar( "xblive_matchEndingSoon", 1 );
}

// ---- the clock and the limits ---------------------------------------------------

// The last minute's beeps and the "ending soon" notifies the music and the
// announcer wait for.
timelimitclock()
{
    level endon( "game_ended" );
    wait 0.05;
    clock = spawn( "script_origin", ( 0, 0, 0 ) );
    while ( game["state"] == "playing" )
    {
        if ( !level.timerstopped && level.timelimit )
        {
            timeleft = maps/mp/gametypes/_globallogic_utils::gettimeremaining() / 1000;
            secs = int( timeleft + 0.5 );
            if ( secs == 601 )
                clientnotify( "notify_10" );
            if ( secs == 301 )
                clientnotify( "notify_5" );
            if ( secs == 60 )
                clientnotify( "notify_1" );
            if ( secs == 12 )
                clientnotify( "notify_count" );
            if ( secs >= 40 && secs <= 60 )
                level notify( "match_ending_soon", "time" );
            if ( secs >= 30 && secs <= 40 )
                level notify( "match_ending_pretty_soon", "time" );
            if ( secs <= 32 )
                level notify( "match_ending_vox" );
            if ( secs <= 10 || ( secs <= 30 && secs % 2 == 0 ) )
            {
                level notify( "match_ending_very_soon", "time" );
                if ( secs == 0 )
                    break;
                clock playsound( "mpl_ui_timer_countdown" );
            }
            if ( timeleft - floor( timeleft ) >= 0.05 )
                wait timeleft - floor( timeleft );
        }
        wait 1.0;
    }
}

timelimitclock_intermission( waittime )
{
    setgameendtime( gettime() + int( waittime * 1000 ) );
    clock = spawn( "script_origin", ( 0, 0, 0 ) );
    if ( waittime >= 10.0 )
        wait waittime - 10.0;
    for ( ;; )
    {
        clock playsound( "mpl_ui_timer_countdown" );
        wait 1.0;
    }
}

// Once a second: the limits from the settings (they may change mid-match),
// then whether one is reached.
updategametypedvars()
{
    level endon( "game_ended" );
    while ( game["state"] == "playing" )
    {
        roundlimit = clamp( getgametypesetting( "roundLimit" ), level.roundlimitmin, level.roundlimitmax );
        if ( roundlimit != level.roundlimit )
        {
            level.roundlimit = roundlimit;
            level notify( "update_roundlimit" );
        }
        timelimit = [[ level.gettimelimit ]]();
        if ( timelimit != level.timelimit )
        {
            level.timelimit = timelimit;
            setdvar( "ui_timelimit", level.timelimit );
            level notify( "update_timelimit" );
        }
        thread checktimelimit();
        scorelimit = clamp( getgametypesetting( "scoreLimit" ), level.scorelimitmin, level.scorelimitmax );
        if ( scorelimit != level.scorelimit )
        {
            level.scorelimit = scorelimit;
            setdvar( "ui_scorelimit", level.scorelimit );
            level notify( "update_scorelimit" );
        }
        thread checkscorelimit();
        if ( isdefined( level.starttime ) && maps/mp/gametypes/_globallogic_utils::gettimeremaining() < 3000 )
        {
            wait 0.1;
            continue;
        }
        wait 1;
    }
}

checktimelimit()
{
    if ( isdefined( level.timelimitoverride ) && level.timelimitoverride )
        return;
    if ( game["state"] != "playing" || level.timelimit <= 0 || level.inprematchperiod || level.timerstopped )
    {
        setgameendtime( 0 );
        return;
    }
    if ( !isdefined( level.starttime ) )
        return;
    timeleft = maps/mp/gametypes/_globallogic_utils::gettimeremaining();
    setgameendtime( gettime() + int( timeleft ) );
    if ( timeleft > 0 )
        return;
    [[ level.ontimelimit ]]();
}

checkscorelimit()
{
    if ( game["state"] != "playing" )
        return 0;
    if ( level.scorelimit <= 0 )
        return 0;
    if ( level.teambased )
    {
        if ( allteamsunderscorelimit() )
            return 0;
    }
    else
    {
        if ( !isplayer( self ) )
            return 0;
        if ( self.pointstowin < level.scorelimit )
            return 0;
    }
    [[ level.onscorelimit ]]();
}

allteamsunderscorelimit()
{
    foreach ( team in level.teams )
    {
        if ( game["teamScores"][team] >= level.scorelimit )
            return 0;
    }
    return 1;
}

checkteamscorelimitsoon( team )
{
    if ( level.scorelimit <= 0 || !level.teambased )
        return;
    if ( maps/mp/gametypes/_globallogic_utils::gettimepassed() < 60000 )
        return;
    if ( maps/mp/gametypes/_globallogic_utils::getestimatedtimeuntilscorelimit( team ) < 1 )
        level notify( "match_ending_soon", "score" );
}

checkplayerscorelimitsoon()
{
    if ( level.scorelimit <= 0 || level.teambased )
        return;
    if ( maps/mp/gametypes/_globallogic_utils::gettimepassed() < 60000 )
        return;
    if ( maps/mp/gametypes/_globallogic_utils::getestimatedtimeuntilscorelimit( undefined ) < 1 )
        level notify( "match_ending_soon", "score" );
}

// ---- the end of a round and of the match ------------------------------------------

endgame( winner, endreasontext )
{
    if ( game["state"] == "postgame" || level.gameended )
        return;
    if ( isdefined( level.onendgame ) )
        [[ level.onendgame ]]( winner );
    setmatchflag( "enable_popups", 0 );
    if ( !isdefined( level.disableoutrovisionset ) || level.disableoutrovisionset == 0 )
        visionsetnaked( "mpOutro", 2.0 );
    setmatchflag( "cg_drawSpectatorMessages", 0 );
    setmatchflag( "game_ended", 1 );
    game["state"] = "postgame";
    level.gameendtime = gettime();
    level.gameended = 1;
    setdvar( "g_gameEnded", 1 );
    level.ingraceperiod = 0;
    level notify( "game_ended" );
    level.allowbattlechatter = 0;
    maps/mp/gametypes/_globallogic_audio::flushdialog();
    foreach ( team in level.teams )
        game["lastroundscore"][team] = getteamscore( team );
    if ( !isdefined( game["overtime_round"] ) || maps/mp/_utility::waslastround() )
    {
        game["roundsplayed"]++;
        game["roundwinner"][game["roundsplayed"]] = winner;
        if ( level.teambased )
            game["roundswon"][winner]++;
    }
    if ( isdefined( winner ) && level.teambased && isdefined( level.teams[winner] ) )
        level.finalkillcam_winner = winner;
    else
        level.finalkillcam_winner = "none";
    setgameendtime( 0 );
    updateplacement();
    updaterankedmatch( winner );
    newtime = gettime();
    foreach ( player in level.players )
    {
        player maps/mp/gametypes/_globallogic_player::freezeplayerforroundend();
        player thread roundenddof( 4.0 );
        player maps/mp/gametypes/_globallogic_ui::freegameplayhudelems();
        player maps/mp/gametypes/_weapons::updateweapontimings( newtime );
        // BO2's: a ranked match (Combat Training) leaves the lobby its
        // After Action Report (lane C).
        if ( level.rankedmatch && !player issplitscreen() )
        {
            if ( isdefined( player.setpromotion ) )
                player setdstat( "AfterActionReportStats", "lobbyPopup", "promotion" );
            else
                player setdstat( "AfterActionReportStats", "lobbyPopup", "summary" );
        }
    }
    maps/mp/_music::setmusicstate( "SILENT" );
    thread maps/mp/_challenges::roundend( winner );
    if ( startnextround( winner, endreasontext ) )
        return;
    if ( !maps/mp/_utility::isoneround() && !level.gameforfeited )
    {
        if ( isdefined( level.onroundendgame ) )
            winner = [[ level.onroundendgame ]]( winner );
        endreasontext = getendreasontext();
    }
    thread maps/mp/_challenges::gameend( winner );
    if ( !isdefined( level.skipgameend ) || !level.skipgameend )
    {
        if ( isdefined( level.preendgamefunction ) )
            thread [[ level.preendgamefunction ]]( level.postroundtime );
        displaygameend( winner, endreasontext );
    }
    if ( maps/mp/_utility::isoneround() )
        maps/mp/gametypes/_globallogic_utils::executepostroundevents();
    level.intermission = 1;
    // BO2's end scoreboard shows the world back in full colour (the outro grade ends with the outcome).
    visionsetnaked( getdvar( "mapname" ), 1.0 );
    foreach ( player in level.players )
    {
        player closemenu();
        player closeingamemenu();
        player notify( "reset_outcome" );
        player thread [[ level.spawnintermission ]]();
        player setclientuivisibilityflag( "hud_visible", 1 );
    }
    if ( isdefined( level.endgamefunction ) )
        level thread [[ level.endgamefunction ]]();
    level notify( "sfade" );
    if ( !isdefined( level.skipgameend ) || !level.skipgameend )
        wait 5.0;
    exitlevel( 0 );
}

// A round of a several-round game type ended: its outcome, then the next round.
startnextround( winner, endreasontext )
{
    if ( maps/mp/_utility::isoneround() )
        return 0;
    displayroundend( winner, endreasontext );
    maps/mp/gametypes/_globallogic_utils::executepostroundevents();
    if ( maps/mp/_utility::waslastround() )
        return 0;
    if ( checkroundswitch() )
        displayroundswitch( winner, endreasontext );
    if ( isdefined( level.nextroundisovertime ) && level.nextroundisovertime )
    {
        if ( !isdefined( game["overtime_round"] ) )
            game["overtime_round"] = 1;
        else
            game["overtime_round"]++;
    }
    game["state"] = "playing";
    level.allowbattlechatter = getgametypesetting( "allowBattleChatter" );
    map_restart( 1 );
    return 1;
}

getendreasontext()
{
    if ( isdefined( level.endreasontext ) )
        return level.endreasontext;
    if ( maps/mp/_utility::hitroundlimit() || maps/mp/_utility::hitroundwinlimit() )
        return game["strings"]["round_limit_reached"];
    if ( maps/mp/_utility::hitscorelimit() )
        return game["strings"]["score_limit_reached"];
    if ( level.forcedend )
    {
        if ( level.hostforcedend )
            return &"MP_HOST_ENDED_GAME";
        return &"MP_ENDED_GAME";
    }
    return game["strings"]["time_limit_reached"];
}

// The round's outcome screen for everyone, then the wait before what follows.
displayroundend( winner, endreasontext )
{
    if ( level.displayroundendtext )
    {
        setmatchflag( "cg_drawSpectatorMessages", 0 );
        foreach ( player in level.players )
        {
            if ( !maps/mp/_utility::waslastround() )
                player notify( "round_ended" );
            if ( !isdefined( player.pers["team"] ) )
            {
                player [[ level.spawnintermission ]]( 1 );
                player closemenu();
                player closeingamemenu();
                continue;
            }
            if ( level.teambased )
                player thread [[ level.onteamoutcomenotify ]]( winner, 1, endreasontext );
            else
                player thread [[ level.onoutcomenotify ]]( winner, 1, endreasontext );
            player maps/mp/gametypes/_globallogic_audio::set_music_on_player( "ROUND_END" );
            player setclientuivisibilityflag( "hud_visible", 0 );
            player setclientuivisibilityflag( "g_compassShowEnemies", 0 );
        }
    }
    if ( maps/mp/_utility::waslastround() )
        roundendwait( level.roundenddelay, 0 );
    else
    {
        thread maps/mp/gametypes/_globallogic_audio::announceroundwinner( winner, level.roundenddelay / 4 );
        roundendwait( level.roundenddelay, 1 );
    }
}

// The match's outcome screen, victory or defeat music, the announcer.
displaygameend( winner, endreasontext )
{
    setmatchflag( "cg_drawSpectatorMessages", 0 );
    foreach ( player in level.players )
    {
        if ( !isdefined( player.pers["team"] ) )
        {
            player [[ level.spawnintermission ]]( 1 );
            player closemenu();
            player closeingamemenu();
            continue;
        }
        if ( level.teambased )
            player thread [[ level.onteamoutcomenotify ]]( winner, 0, endreasontext );
        else
        {
            player thread [[ level.onoutcomenotify ]]( winner, 0, endreasontext );
            if ( isdefined( winner ) && player == winner )
                player maps/mp/gametypes/_globallogic_audio::set_music_on_player( game["music"]["victory_" + player.team] );
            else
                player maps/mp/gametypes/_globallogic_audio::set_music_on_player( "LOSE" );
        }
        player setclientuivisibilityflag( "hud_visible", 0 );
        player setclientuivisibilityflag( "g_compassShowEnemies", 0 );
    }
    if ( level.teambased )
    {
        thread maps/mp/gametypes/_globallogic_audio::announcegamewinner( winner, level.postroundtime / 2 );
        foreach ( player in level.players )
        {
            team = player.pers["team"];
            if ( winner == "tie" )
                player maps/mp/gametypes/_globallogic_audio::set_music_on_player( "DRAW" );
            else if ( winner == team )
                player maps/mp/gametypes/_globallogic_audio::set_music_on_player( game["music"]["victory_" + player.team] );
            else
                player maps/mp/gametypes/_globallogic_audio::set_music_on_player( "LOSE" );
        }
    }
    roundendwait( level.postroundtime, 1 );
}

// Halftime, overtime or an intermission between rounds: sides switch.
displayroundswitch( winner, endreasontext )
{
    switchtype = level.halftimetype;
    if ( switchtype == "halftime" )
    {
        if ( isdefined( level.nextroundisovertime ) && level.nextroundisovertime )
            switchtype = "overtime";
        else if ( level.roundlimit )
        {
            if ( game["roundsplayed"] * 2 == level.roundlimit )
                switchtype = "halftime";
            else
                switchtype = "intermission";
        }
        else if ( level.scorelimit )
        {
            if ( game["roundsplayed"] == level.scorelimit - 1 )
                switchtype = "halftime";
            else
                switchtype = "intermission";
        }
        else
            switchtype = "intermission";
    }
    leaderdialog = maps/mp/gametypes/_globallogic_audio::getroundswitchdialog( switchtype );
    foreach ( player in level.players )
    {
        if ( !isdefined( player.pers["team"] ) )
        {
            player [[ level.spawnintermission ]]( 1 );
            player closemenu();
            player closeingamemenu();
            continue;
        }
        player maps/mp/gametypes/_globallogic_audio::leaderdialogonplayer( leaderdialog );
        player maps/mp/gametypes/_globallogic_audio::set_music_on_player( "ROUND_SWITCH" );
        player thread [[ level.onteamoutcomenotify ]]( switchtype, 0, level.halftimesubcaption );
        player setclientuivisibilityflag( "hud_visible", 0 );
    }
    roundendwait( level.halftimeroundenddelay, 0 );
}

checkroundswitch()
{
    if ( !isdefined( level.roundswitch ) || !level.roundswitch )
        return 0;
    if ( !isdefined( level.onroundswitch ) )
        return 0;
    if ( game["roundsplayed"] % level.roundswitch == 0 )
    {
        [[ level.onroundswitch ]]();
        return 1;
    }
    return 0;
}

// Wait out every player's outcome notify, then the delay (the match bonus
// lands halfway when there is one).
roundendwait( defaultdelay, matchbonus )
{
    waitfornotifies();
    if ( !matchbonus )
    {
        wait defaultdelay;
        level notify( "round_end_done" );
        return;
    }
    wait defaultdelay / 2;
    level notify( "give_match_bonus" );
    wait defaultdelay / 2;
    waitfornotifies();
    level notify( "round_end_done" );
}

waitfornotifies()
{
    for ( ;; )
    {
        busy = 0;
        foreach ( player in level.players )
        {
            if ( isdefined( player.doingnotify ) && player.doingnotify )
                busy = 1;
        }
        if ( !busy )
            return;
        wait 0.5;
    }
}

roundenddof( time )
{
    self setdepthoffield( 0, 128, 512, 4000, 6, 1.8 );
}

// The host ends the match from the pause menu.
forceend( hostsucks )
{
    if ( !isdefined( hostsucks ) )
        hostsucks = 0;
    if ( level.hostforcedend || level.forcedend )
        return;
    winner = undefined;
    if ( level.teambased )
        winner = determineteamwinnerbygamestat( "teamScores" );
    else
        winner = maps/mp/gametypes/_globallogic_score::gethighestscoringplayer();
    level.forcedend = 1;
    level.hostforcedend = 1;
    if ( hostsucks )
        endstring = &"MP_HOST_SUCKS";
    else
        endstring = &"MP_HOST_ENDED_GAME";
    setmatchflag( "disableIngameMenu", 1 );
    makedvarserverinfo( "ui_text_endreason", endstring );
    setdvar( "ui_text_endreason", endstring );
    thread endgame( winner, endstring );
}

killserverpc()
{
    if ( level.hostforcedend || level.forcedend )
        return;
    winner = undefined;
    if ( level.teambased )
        winner = determineteamwinnerbygamestat( "teamScores" );
    else
        winner = maps/mp/gametypes/_globallogic_score::gethighestscoringplayer();
    level.forcedend = 1;
    level.hostforcedend = 1;
    level.killserver = 1;
    thread endgame( winner, &"MP_HOST_ENDED_GAME" );
}

listenforgameend()
{
    self waittill( "host_sucks_end_game" );
    level.skipvote = 1;
    if ( !level.gameended )
        level thread forceend( 1 );
}

// ---- who won ------------------------------------------------------------------

compareteambygamestat( gamestat, teama, teamb, previous_winner_score )
{
    if ( teama == "tie" )
    {
        if ( previous_winner_score < game[gamestat][teamb] )
            return teamb;
        return "tie";
    }
    if ( game[gamestat][teama] == game[gamestat][teamb] )
        return "tie";
    if ( game[gamestat][teamb] > game[gamestat][teama] )
        return teamb;
    return teama;
}

determineteamwinnerbygamestat( gamestat )
{
    keys = getarraykeys( level.teams );
    winner = keys[0];
    best = game[gamestat][winner];
    for ( index = 1; index < keys.size; index++ )
    {
        winner = compareteambygamestat( gamestat, winner, keys[index], best );
        if ( winner != "tie" )
            best = game[gamestat][winner];
    }
    return winner;
}

compareteambyteamscore( teama, teamb, previous_winner_score )
{
    scoreb = [[ level._getteamscore ]]( teamb );
    if ( teama == "tie" )
    {
        if ( previous_winner_score < scoreb )
            return teamb;
        return "tie";
    }
    scorea = [[ level._getteamscore ]]( teama );
    if ( scoreb == scorea )
        return "tie";
    if ( scoreb > scorea )
        return teamb;
    return teama;
}

determineteamwinnerbyteamscore()
{
    keys = getarraykeys( level.teams );
    winner = keys[0];
    best = [[ level._getteamscore ]]( winner );
    for ( index = 1; index < keys.size; index++ )
    {
        winner = compareteambyteamscore( winner, keys[index], best );
        if ( winner != "tie" )
            best = [[ level._getteamscore ]]( winner );
    }
    return winner;
}

// ---- placement (the scoreboard order) ---------------------------------------------

updateplacement()
{
    if ( !level.players.size )
        return;
    all = [];
    foreach ( player in level.players )
    {
        if ( isdefined( level.teams[player.team] ) )
            all[all.size] = player;
    }
    // Insertion sort: most points first (score in team games, points to win
    // otherwise), fewer deaths breaking a tie.
    for ( i = 1; i < all.size; i++ )
    {
        player = all[i];
        mine = placementscore( player );
        j = i - 1;
        while ( j >= 0 && ( mine > placementscore( all[j] ) || ( mine == placementscore( all[j] ) && player.deaths < all[j].deaths ) ) )
        {
            all[j + 1] = all[j];
            j--;
        }
        all[j + 1] = player;
    }
    level.placement["all"] = all;
    updateteamplacement();
}

placementscore( player )
{
    if ( level.teambased )
        return player.score;
    return player.pointstowin;
}

updateteamplacement()
{
    if ( !level.teambased )
        return;
    placement = [];
    foreach ( team in level.teams )
        placement[team] = [];
    foreach ( player in level.placement["all"] )
    {
        team = player.pers["team"];
        if ( isdefined( placement[team] ) )
            placement[team][placement[team].size] = player;
    }
    foreach ( team in level.teams )
        level.placement[team] = placement[team];
}

getplacementforplayer( player )
{
    updateplacement();
    for ( index = 0; index < level.placement["all"].size; index++ )
    {
        if ( level.placement["all"][index] == player )
            return index + 1;
    }
    return -1;
}

istopscoringplayer( player )
{
    updateplacement();
    if ( level.placement["all"].size == 0 )
        return 0;
    top = placementscore( level.placement["all"][0] );
    foreach ( other in level.placement["all"] )
    {
        mine = placementscore( other );
        if ( mine == 0 || top > mine )
            return 0;
        if ( other == self )
            return 1;
    }
    return 0;
}

removedisconnectedplayerfromplacement()
{
    kept = [];
    found = 0;
    foreach ( player in level.placement["all"] )
    {
        if ( player == self )
            found = 1;
        else
            kept[kept.size] = player;
    }
    if ( !found )
        return;
    level.placement["all"] = kept;
    updateteamplacement();
    if ( level.teambased )
        return;
    foreach ( player in level.placement["all"] )
        player notify( "update_outcome" );
}

// ---- team status: who is alive, lives left, the "last alive" events ---------------

initteamvariables( team )
{
    if ( !isdefined( level.alivecount ) )
        level.alivecount = [];
    level.alivecount[team] = 0;
    level.lastalivecount[team] = 0;
    if ( !isdefined( game["everExisted"] ) )
        game["everExisted"] = [];
    if ( !isdefined( game["everExisted"][team] ) )
        game["everExisted"][team] = 0;
    level.everexisted[team] = 0;
    level.wavedelay[team] = 0;
    level.lastwave[team] = 0;
    level.waveplayerspawnindex[team] = 0;
    resetteamvariables( team );
}

resetteamvariables( team )
{
    level.playercount[team] = 0;
    level.botscount[team] = 0;
    level.lastalivecount[team] = level.alivecount[team];
    level.alivecount[team] = 0;
    level.playerlives[team] = 0;
    level.aliveplayers[team] = [];
    level.deadplayers[team] = [];
    level.squads[team] = [];
    level.spawnqueuemodified[team] = 0;
}

assertteamvariables()
{
}

updateteamstatus()
{
    level notify( "updating_team_status" );
    level endon( "updating_team_status" );
    level endon( "game_ended" );
    waittillframeend;
    wait 0;
    if ( game["state"] == "postgame" )
        return;
    resettimeout();
    foreach ( team in level.teams )
        resetteamvariables( team );
    level.activeplayers = [];
    foreach ( player in level.players )
    {
        team = player.team;
        if ( team == "spectator" || !isdefined( player.class ) || player.class == "" )
            continue;
        level.playercount[team]++;
        if ( isdefined( player.pers["isBot"] ) )
            level.botscount[team]++;
        if ( player.sessionstate == "playing" )
        {
            level.alivecount[team]++;
            level.playerlives[team]++;
            player.spawnqueueindex = -1;
            if ( isalive( player ) )
            {
                level.aliveplayers[team][level.aliveplayers[team].size] = player;
                level.activeplayers[level.activeplayers.size] = player;
            }
            else
                level.deadplayers[team][level.deadplayers[team].size] = player;
            continue;
        }
        level.deadplayers[team][level.deadplayers[team].size] = player;
        if ( player maps/mp/gametypes/_globallogic_spawn::mayspawn() )
            level.playerlives[team]++;
    }
    alive = totalalivecount();
    if ( alive > level.maxplayercount )
        level.maxplayercount = alive;
    foreach ( team in level.teams )
    {
        if ( level.alivecount[team] )
        {
            game["everExisted"][team] = 1;
            level.everexisted[team] = 1;
        }
        sortdeadplayers( team );
    }
    level updategameevents();
}

// The queue of the dead, longest dead first (game types that respawn in turn).
sortdeadplayers( team )
{
    if ( !level.playerqueuedrespawn )
        return;
    dead = level.deadplayers[team];
    for ( i = 1; i < dead.size; i++ )
    {
        player = dead[i];
        j = i - 1;
        while ( j >= 0 && player.deathtime < dead[j].deathtime )
        {
            dead[j + 1] = dead[j];
            j--;
        }
        dead[j + 1] = player;
    }
    level.deadplayers[team] = dead;
    for ( i = 0; i < dead.size; i++ )
    {
        if ( dead[i].spawnqueueindex != i )
            level.spawnqueuemodified[team] = 1;
        dead[i].spawnqueueindex = i;
    }
}

totalalivecount()
{
    count = 0;
    foreach ( team in level.teams )
        count = count + level.alivecount[team];
    return count;
}

totalplayerlives()
{
    count = 0;
    foreach ( team in level.teams )
        count = count + level.playerlives[team];
    return count;
}

totalplayercount()
{
    count = 0;
    foreach ( team in level.teams )
        count = count + level.playercount[team];
    return count;
}

atleasttwoteams()
{
    teams = 0;
    foreach ( team in level.teams )
    {
        if ( level.playercount[team] != 0 )
            teams++;
    }
    return teams >= 2;
}

isteamalldead( team )
{
    return level.everexisted[team] && !level.alivecount[team] && !level.playerlives[team];
}

areallteamsdead()
{
    foreach ( team in level.teams )
    {
        if ( !isteamalldead( team ) )
            return 0;
    }
    return 1;
}

getlastteamalive()
{
    alive = 0;
    existed = 0;
    last = undefined;
    foreach ( team in level.teams )
    {
        if ( !level.everexisted[team] )
            continue;
        existed++;
        if ( !isteamalldead( team ) )
        {
            last = team;
            alive++;
        }
    }
    if ( existed > 1 && alive == 1 )
        return last;
    return undefined;
}

dodeadeventupdates()
{
    if ( level.teambased )
    {
        if ( areallteamsdead() )
        {
            [[ level.ondeadevent ]]( "all" );
            return 1;
        }
        if ( !isdefined( level.ondeadevent ) )
        {
            last = getlastteamalive();
            if ( isdefined( last ) )
            {
                [[ level.onlastteamaliveevent ]]( last );
                return 1;
            }
            return 0;
        }
        foreach ( team in level.teams )
        {
            if ( isteamalldead( team ) )
            {
                [[ level.ondeadevent ]]( team );
                return 1;
            }
        }
        return 0;
    }
    if ( totalalivecount() == 0 && totalplayerlives() == 0 && level.maxplayercount > 1 )
    {
        [[ level.ondeadevent ]]( "all" );
        return 1;
    }
    return 0;
}

isonlyoneleftaliveonteam( team )
{
    return level.lastalivecount[team] > 1 && level.alivecount[team] == 1 && level.playerlives[team] == 1;
}

doonelefteventupdates()
{
    if ( level.teambased )
    {
        foreach ( team in level.teams )
        {
            if ( isonlyoneleftaliveonteam( team ) )
            {
                [[ level.ononeleftevent ]]( team );
                return 1;
            }
        }
        return 0;
    }
    if ( totalalivecount() == 1 && totalplayerlives() == 1 && level.maxplayercount > 1 )
    {
        [[ level.ononeleftevent ]]( "all" );
        return 1;
    }
    return 0;
}

dospawnqueueupdates()
{
    foreach ( team in level.teams )
    {
        if ( level.spawnqueuemodified[team] )
            [[ level.onalivecountchange ]]( team );
    }
}

// A local match never forfeits (no ranked play): only the round events of
// game types with lives (Search and Destroy) or queued respawns.
updategameevents()
{
    if ( !level.playerqueuedrespawn && !level.numlives && !level.inovertime )
        return;
    if ( level.ingraceperiod )
        return;
    if ( level.playerqueuedrespawn )
        dospawnqueueupdates();
    if ( dodeadeventupdates() )
        return;
    doonelefteventupdates();
}

// ---- wave respawns ------------------------------------------------------------

anyteamhaswavedelay()
{
    foreach ( team in level.teams )
    {
        if ( level.wavedelay[team] )
            return 1;
    }
    return 0;
}

notifyteamwavespawn( team, time )
{
    if ( time - level.lastwave[team] > level.wavedelay[team] * 1000 )
    {
        level notify( "wave_respawn_" + team );
        level.lastwave[team] = time;
        level.waveplayerspawnindex[team] = 0;
    }
}

wavespawntimer()
{
    level endon( "game_ended" );
    while ( game["state"] == "playing" )
    {
        time = gettime();
        foreach ( team in level.teams )
            notifyteamwavespawn( team, time );
        wait 0.05;
    }
}

// ---- small answers ------------------------------------------------------------

registerfriendlyfiredelay( dvarstring, defaultvalue, minvalue, maxvalue )
{
    dvarstring = "scr_" + dvarstring + "_friendlyFireDelayTime";
    if ( getdvar( dvarstring ) == "" )
        setdvar( dvarstring, defaultvalue );
    if ( getdvarint( dvarstring ) > maxvalue )
        setdvar( dvarstring, maxvalue );
    else if ( getdvarint( dvarstring ) < minvalue )
        setdvar( dvarstring, minvalue );
    level.friendlyfiredelaytime = getdvarint( dvarstring );
}

// The scorestreaks a player brings (the end screen lists them).
getkillstreaks( player )
{
    killstreak = [];
    for ( index = 0; index < level.maxkillstreaks; index++ )
        killstreak[index] = "killstreak_null";
    if ( isplayer( player ) && !level.oldschool && level.disableclassselection != 1 && !isdefined( player.pers["isBot"] ) && isdefined( player.killstreak ) )
    {
        count = 0;
        for ( index = 0; index < level.maxkillstreaks; index++ )
        {
            if ( isdefined( player.killstreak[index] ) )
            {
                killstreak[count] = player.killstreak[index];
                count++;
            }
        }
    }
    return killstreak;
}

getgamelength()
{
    if ( !level.timelimit || level.forcedend )
        return min( maps/mp/gametypes/_globallogic_utils::gettimepassed() / 1000, 1200 );
    return level.timelimit * 60;
}

gethighestscore()
{
    best = -999999999;
    foreach ( player in level.players )
    {
        if ( player.score > best )
            best = player.score;
    }
    return best;
}

getnexthighestscore( score )
{
    best = -999999999;
    foreach ( player in level.players )
    {
        if ( player.score < score && player.score > best )
            best = player.score;
    }
    return best;
}

getteamscoreratio()
{
    team = self.team;
    if ( !level.teambased || !isdefined( level.teams[team] ) )
        return 0;
    mine = getteamscore( team );
    others = 0;
    count = 0;
    foreach ( other in level.teams )
    {
        if ( other == team )
            continue;
        others = others + getteamscore( other );
        count++;
    }
    if ( count == 0 || others == 0 )
        return mine;
    return mine / ( others / count );
}

getcurrentgamemode()
{
    return "publicmatch";
}

hostidledout()
{
    return 0;
}

// Online statistics and reports: none in a local match.
incrementmatchcompletionstat( gamemode, playedorhosted, stat )
{
}

setmatchcompletionstat( gamemode, playedorhosted, stat )
{
}

gamehistoryplayerkicked()
{
}

gamehistoryplayerquit()
{
}

bbplayermatchend( gamelength, endreasonstring, gameover )
{
}

sendafteractionreport()
{
}

precache_mp_leaderboards()
{
}

// Combat Training (lane C). BO2 matched him with other players and set
// bots against them one for one (_bot.gsc's ranked "comp stomp"); with no
// one else here, the playlist's bots fill both teams by BO2's own
// custom-game bot setup (bot_friends, bot_enemies, bot_difficulty). The
// engine keeps BO2's ranked bot branch off (sv_botsoak).
combattrainingbots()
{
    if ( getdvarint( "bot_friends" ) <= 0 && getdvarint( "bot_enemies" ) <= 0 )
        return;
    maps/mp/bots/_bot::bot_wait_for_host();
    maps/mp/bots/_bot::bot_set_difficulty();
    level thread maps/mp/bots/_bot::bot_local_think();
}

updaterankedmatch( winner )
{
    // BO2's match bonus XP (win, loss, tie by time played; lane C). XP
    // counts only in a ranked match (Combat Training).
    if ( !level.wagermatch && !sessionmodeiszombiesgame() )
        maps/mp/gametypes/_globallogic_score::updatematchbonusscores( winner );
}

settopplayerstats()
{
}

settopteamstats( winner )
{
}

resetoutcomeforallplayers()
{
    foreach ( player in level.players )
        player notify( "reset_outcome" );
}

forcedebughostmigration()
{
}

// BO2's own postroundfinalkillcam (_killcam) notifies dofinalkillcam, then waits for
// "final_killcam_done". With no kill to show dofinalkillcam sends that at once, before
// our wait is registered, and endgame stalled forever (no intermission, no scoreboard).
// Here the wait happens only while the final killcam is really running.
postroundfinalkillcam()
{
    if ( isdefined( level.sidebet ) && level.sidebet )
        return;
    // The host ended it: no final killcam (BO2 goes from the outcome to the scoreboard).
    if ( level.hostforcedend || level.forcedend )
        return;
    level notify( "play_final_killcam" );
    resetoutcomeforallplayers();
    if ( isdefined( level.infinalkillcam ) && level.infinalkillcam )
        level waittill( "final_killcam_done" );
}
