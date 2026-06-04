const POLL_INTERVAL_MS = 1200;
const REMOTE_SPRITE_BASE =
  "https://raw.githubusercontent.com/PokeAPI/sprites/master/sprites/pokemon/other/official-artwork";
const MOVE_KEYS_BY_ID = [
  null,
  "POUND",
  "KARATE_CHOP",
  "DOUBLE_SLAP",
  "COMET_PUNCH",
  "MEGA_PUNCH",
  "PAY_DAY",
  "FIRE_PUNCH",
  "ICE_PUNCH",
  "THUNDER_PUNCH",
  "SCRATCH",
  "VICE_GRIP",
  "GUILLOTINE",
  "RAZOR_WIND",
  "SWORDS_DANCE",
  "CUT",
  "GUST",
  "WING_ATTACK",
  "WHIRLWIND",
  "FLY",
  "BIND",
  "SLAM",
  "VINE_WHIP",
  "STOMP",
  "DOUBLE_KICK",
  "MEGA_KICK",
  "JUMP_KICK",
  "ROLLING_KICK",
  "SAND_ATTACK",
  "HEADBUTT",
  "HORN_ATTACK",
  "FURY_ATTACK",
  "HORN_DRILL",
  "TACKLE",
  "BODY_SLAM",
  "WRAP",
  "TAKE_DOWN",
  "THRASH",
  "DOUBLE_EDGE",
  "TAIL_WHIP",
  "POISON_STING",
  "TWINEEDLE",
  "PIN_MISSILE",
  "LEER",
  "BITE",
  "GROWL",
  "ROAR",
  "SING",
  "SUPERSONIC",
  "SONIC_BOOM",
  "DISABLE",
  "ACID",
  "EMBER",
  "FLAMETHROWER",
  "MIST",
  "WATER_GUN",
  "HYDRO_PUMP",
  "SURF",
  "ICE_BEAM",
  "BLIZZARD",
  "PSYBEAM",
  "BUBBLE_BEAM",
  "AURORA_BEAM",
  "HYPER_BEAM",
  "PECK",
  "DRILL_PECK",
  "SUBMISSION",
  "LOW_KICK",
  "COUNTER",
  "SEISMIC_TOSS",
  "STRENGTH",
  "ABSORB",
  "MEGA_DRAIN",
  "LEECH_SEED",
  "GROWTH",
  "RAZOR_LEAF",
  "SOLAR_BEAM",
  "POISON_POWDER",
  "STUN_SPORE",
  "SLEEP_POWDER",
  "PETAL_DANCE",
  "STRING_SHOT",
  "DRAGON_RAGE",
  "FIRE_SPIN",
  "THUNDER_SHOCK",
  "THUNDERBOLT",
  "THUNDER_WAVE",
  "THUNDER",
  "ROCK_THROW",
  "EARTHQUAKE",
  "FISSURE",
  "DIG",
  "TOXIC",
  "CONFUSION",
  "PSYCHIC",
  "HYPNOSIS",
  "MEDITATE",
  "AGILITY",
  "QUICK_ATTACK",
  "RAGE",
  "TELEPORT",
  "NIGHT_SHADE",
  "MIMIC",
  "SCREECH",
  "DOUBLE_TEAM",
  "RECOVER",
  "HARDEN",
  "MINIMIZE",
  "SMOKESCREEN",
  "CONFUSE_RAY",
  "WITHDRAW",
  "DEFENSE_CURL",
  "BARRIER",
  "LIGHT_SCREEN",
  "HAZE",
  "REFLECT",
  "FOCUS_ENERGY",
  "BIDE",
  "METRONOME",
  "MIRROR_MOVE",
  "SELF_DESTRUCT",
  "EGG_BOMB",
  "LICK",
  "SMOG",
  "SLUDGE",
  "BONE_CLUB",
  "FIRE_BLAST",
  "WATERFALL",
  "CLAMP",
  "SWIFT",
  "SKULL_BASH",
  "SPIKE_CANNON",
  "CONSTRICT",
  "AMNESIA",
  "KINESIS",
  "SOFT_BOILED",
  "HI_JUMP_KICK",
  "GLARE",
  "DREAM_EATER",
  "POISON_GAS",
  "BARRAGE",
  "LEECH_LIFE",
  "LOVELY_KISS",
  "SKY_ATTACK",
  "TRANSFORM",
  "BUBBLE",
  "DIZZY_PUNCH",
  "SPORE",
  "FLASH",
  "PSYWAVE",
  "SPLASH",
  "ACID_ARMOR",
  "CRABHAMMER",
  "EXPLOSION",
  "FURY_SWIPES",
  "BONEMERANG",
  "REST",
  "ROCK_SLIDE",
  "HYPER_FANG",
  "SHARPEN",
  "CONVERSION",
  "TRI_ATTACK",
  "SUPER_FANG",
  "SLASH",
  "SUBSTITUTE",
  "STRUGGLE",
  "SKETCH",
  "TRIPLE_KICK",
  "THIEF",
  "SPIDER_WEB",
  "MIND_READER",
  "NIGHTMARE",
  "FLAME_WHEEL",
  "SNORE",
  "CURSE",
  "FLAIL",
  "CONVERSION_2",
  "AEROBLAST",
  "COTTON_SPORE",
  "REVERSAL",
  "SPITE",
  "POWDER_SNOW",
  "PROTECT",
  "MACH_PUNCH",
  "SCARY_FACE",
  "FAINT_ATTACK",
  "SWEET_KISS",
  "BELLY_DRUM",
  "SLUDGE_BOMB",
  "MUD_SLAP",
  "OCTAZOOKA",
  "SPIKES",
  "ZAP_CANNON",
  "FORESIGHT",
  "DESTINY_BOND",
  "PERISH_SONG",
  "ICY_WIND",
  "DETECT",
  "BONE_RUSH",
  "LOCK_ON",
  "OUTRAGE",
  "SANDSTORM",
  "GIGA_DRAIN",
  "ENDURE",
  "CHARM",
  "ROLLOUT",
  "FALSE_SWIPE",
  "SWAGGER",
  "MILK_DRINK",
  "SPARK",
  "FURY_CUTTER",
  "STEEL_WING",
  "MEAN_LOOK",
  "ATTRACT",
  "SLEEP_TALK",
  "HEAL_BELL",
  "RETURN",
  "PRESENT",
  "FRUSTRATION",
  "SAFEGUARD",
  "PAIN_SPLIT",
  "SACRED_FIRE",
  "MAGNITUDE",
  "DYNAMIC_PUNCH",
  "MEGAHORN",
  "DRAGON_BREATH",
  "BATON_PASS",
  "ENCORE",
  "PURSUIT",
  "RAPID_SPIN",
  "SWEET_SCENT",
  "IRON_TAIL",
  "METAL_CLAW",
  "VITAL_THROW",
  "MORNING_SUN",
  "SYNTHESIS",
  "MOONLIGHT",
  "HIDDEN_POWER",
  "CROSS_CHOP",
  "TWISTER",
  "RAIN_DANCE",
  "SUNNY_DAY",
  "CRUNCH",
  "MIRROR_COAT",
  "PSYCH_UP",
  "EXTREME_SPEED",
  "ANCIENT_POWER",
  "SHADOW_BALL",
  "FUTURE_SIGHT",
  "ROCK_SMASH",
  "WHIRLPOOL",
  "BEAT_UP",
  "FAKE_OUT",
  "UPROAR",
  "STOCKPILE",
  "SPIT_UP",
  "SWALLOW",
  "HEAT_WAVE",
  "HAIL",
  "TORMENT",
  "FLATTER",
  "WILL_O_WISP",
  "MEMENTO",
  "FACADE",
  "FOCUS_PUNCH",
  "SMELLING_SALT",
  "FOLLOW_ME",
  "NATURE_POWER",
  "CHARGE",
  "TAUNT",
  "HELPING_HAND",
  "TRICK",
  "ROLE_PLAY",
  "WISH",
  "ASSIST",
  "INGRAIN",
  "SUPERPOWER",
  "MAGIC_COAT",
  "RECYCLE",
  "REVENGE",
  "BRICK_BREAK",
  "YAWN",
  "KNOCK_OFF",
  "ENDEAVOR",
  "ERUPTION",
  "SKILL_SWAP",
  "IMPRISON",
  "REFRESH",
  "GRUDGE",
  "SNATCH",
  "SECRET_POWER",
  "DIVE",
  "ARM_THRUST",
  "CAMOUFLAGE",
  "TAIL_GLOW",
  "LUSTER_PURGE",
  "MIST_BALL",
  "FEATHER_DANCE",
  "TEETER_DANCE",
  "BLAZE_KICK",
  "MUD_SPORT",
  "ICE_BALL",
  "NEEDLE_ARM",
  "SLACK_OFF",
  "HYPER_VOICE",
  "POISON_FANG",
  "CRUSH_CLAW",
  "BLAST_BURN",
  "HYDRO_CANNON",
  "METEOR_MASH",
  "ASTONISH",
  "WEATHER_BALL",
  "AROMATHERAPY",
  "FAKE_TEARS",
  "AIR_CUTTER",
  "OVERHEAT",
  "ODOR_SLEUTH",
  "ROCK_TOMB",
  "SILVER_WIND",
  "METAL_SOUND",
  "GRASS_WHISTLE",
  "TICKLE",
  "COSMIC_POWER",
  "WATER_SPOUT",
  "SIGNAL_BEAM",
  "SHADOW_PUNCH",
  "EXTRASENSORY",
  "SKY_UPPERCUT",
  "SAND_TOMB",
  "SHEER_COLD",
  "MUDDY_WATER",
  "BULLET_SEED",
  "AERIAL_ACE",
  "ICICLE_SPEAR",
  "IRON_DEFENSE",
  "BLOCK",
  "HOWL",
  "DRAGON_CLAW",
  "FRENZY_PLANT",
  "BULK_UP",
  "BOUNCE",
  "MUD_SHOT",
  "POISON_TAIL",
  "COVET",
  "VOLT_TACKLE",
  "MAGICAL_LEAF",
  "WATER_SPORT",
  "CALM_MIND",
  "LEAF_BLADE",
  "DRAGON_DANCE",
  "ROCK_BLAST",
  "SHOCK_WAVE",
  "WATER_PULSE",
  "DOOM_DESIRE",
  "PSYCHO_BOOST",
];
const MOVE_NAME_OVERRIDES = {
  CONVERSION_2: "Conversion 2",
  DOUBLE_EDGE: "Double-Edge",
  HI_JUMP_KICK: "Hi Jump Kick",
  LOCK_ON: "Lock-On",
  MUD_SLAP: "Mud-Slap",
  WILL_O_WISP: "Will-O-Wisp",
};

const state = {
  lastOkAt: 0,
  currentSignature: "",
};

const overlayRoot = document.getElementById("overlay-root");
const partyGrid = document.getElementById("party-grid");
const emptyState = document.getElementById("empty-state");
const overlayTitle = document.getElementById("overlay-title");
const overlaySubtitle = document.getElementById("overlay-subtitle");
const overlayStatus = document.getElementById("overlay-status");
const battlePanel = document.getElementById("battle-panel");
const battleKicker = document.getElementById("battle-kicker");
const battleName = document.getElementById("battle-name");
const battleLevel = document.getElementById("battle-level");
const battleHp = document.getElementById("battle-hp");
const battleCatch = document.getElementById("battle-catch");
const battleHpFill = document.getElementById("battle-hp-fill");
const areaPanel = document.getElementById("area-panel");
const areaName = document.getElementById("area-name");
const areaMapId = document.getElementById("area-map-id");
const encounterList = document.getElementById("encounter-list");
const partyCardTemplate = document.getElementById("party-card-template");
const encounterMethodTemplate = document.getElementById("encounter-method-template");

initLayout();
tick();
window.setInterval(tick, POLL_INTERVAL_MS);

async function tick() {
  try {
    const response = await fetch(`/api/snapshot?ts=${Date.now()}`, { cache: "no-store" });
    if (!response.ok) {
      throw new Error(`snapshot ${response.status}`);
    }

    const snapshot = await response.json();
    const signature = JSON.stringify(snapshot);
    if (signature !== state.currentSignature) {
      state.currentSignature = signature;
      renderSnapshot(snapshot);
    }

    state.lastOkAt = Date.now();
    setStatus("live", "live");
  } catch (error) {
    const stale = state.lastOkAt && Date.now() - state.lastOkAt < 10_000;
    setStatus(stale ? "stale" : "offline", stale ? "waiting for refresh" : "offline");
    if (!state.currentSignature) {
      renderEmpty("Waiting for a local snapshot export.");
    }
  }
}

function initLayout() {
  const params = new URLSearchParams(window.location.search);
  const transparent = truthy(params.get("transparent"));
  const compact = truthy(params.get("compact"));
  const layout = params.get("layout") || "column";
  const align = params.get("align") || "left";
  const showHeader = !truthy(params.get("hideHeader"));

  document.body.dataset.transparent = transparent ? "true" : "false";
  overlayRoot.dataset.transparent = transparent ? "true" : "false";
  overlayRoot.dataset.compact = compact ? "true" : "false";
  overlayRoot.dataset.layout = ["column", "row", "stack"].includes(layout)
    ? layout
    : "column";
  overlayRoot.dataset.align = ["left", "center", "right"].includes(align) ? align : "left";
  overlayRoot.dataset.header = showHeader ? "visible" : "hidden";
}

function renderSnapshot(snapshot) {
  const addon = snapshot?.addon;
  const fireRed = addon?.data?.type === "fire_red" ? addon.data.payload : null;

  if (!fireRed?.party?.length) {
    overlayTitle.textContent = addon?.display_name || snapshot?.rom?.title || "tinyBird Overlay";
    overlaySubtitle.textContent = fireRed
      ? "Supported addon found, but no live party members were exported yet."
      : "Load FireRed in the desktop app to populate the party overlay.";
    renderEmpty("No party payload is available yet.");
    renderBattlePanel(null);
    renderAreaPanel(null);
    return;
  }

  const rom = snapshot?.rom;
  overlayTitle.textContent = addon.display_name || "Party Overlay";
  overlaySubtitle.textContent = [rom?.title, rom?.game_code].filter(Boolean).join("  ");

  emptyState.classList.remove("is-visible");
  renderBattlePanel(fireRed.battle);
  renderAreaPanel(fireRed.area);
  partyGrid.innerHTML = "";

  for (const member of fireRed.party) {
    partyGrid.appendChild(renderPartyCard(member));
  }
}

function renderBattlePanel(battle) {
  if (!battle?.opponent) {
    battlePanel.classList.remove("is-visible");
    return;
  }

  const opponent = battle.opponent;
  const hpRatio = opponent.max_hp > 0 ? opponent.current_hp / opponent.max_hp : 0;
  battlePanel.classList.add("is-visible");
  battleKicker.textContent = `${battle.battle_kind || "Wild"} Battle`;
  battleName.textContent = opponent.species_name || `#${opponent.species_id}`;
  battleLevel.textContent = `Lv ${opponent.level}`;
  battleHp.textContent = `${opponent.current_hp}/${opponent.max_hp} HP`;
  battleCatch.textContent = battle.catchable
    ? `Catch ${opponent.catch_rate ?? "?"}`
    : "Not catchable";
  battleCatch.dataset.catchable = battle.catchable ? "true" : "false";
  battleHpFill.style.width = `${Math.max(0, Math.min(100, hpRatio * 100))}%`;
  battleHpFill.classList.toggle("is-low", hpRatio <= 0.2);
  battleHpFill.classList.toggle("is-mid", hpRatio > 0.2 && hpRatio <= 0.5);
}

function renderAreaPanel(area) {
  if (!area) {
    areaPanel.classList.remove("is-visible");
    encounterList.innerHTML = "";
    return;
  }

  areaPanel.classList.add("is-visible");
  areaName.textContent = area.name || "Unknown Area";
  areaMapId.textContent = `${area.map_key || "MAP_UNKNOWN"}  ${area.map_group}:${area.map_num}`;
  encounterList.innerHTML = "";

  const groups = area.encounter_groups || [];
  if (!groups.length) {
    const empty = document.createElement("p");
    empty.className = "encounter-empty";
    empty.textContent = "No wild encounters listed for this area yet.";
    encounterList.appendChild(empty);
    return;
  }

  for (const group of groups) {
    const fragment = encounterMethodTemplate.content.cloneNode(true);
    const name = fragment.querySelector(".encounter-method-name");
    const rate = fragment.querySelector(".encounter-rate");
    const table = fragment.querySelector(".encounter-table");

    name.textContent = group.method;
    rate.textContent = `Area rate ${group.encounter_rate}`;

    for (const entry of group.entries || []) {
      table.appendChild(renderEncounterRow(entry));
    }

    encounterList.appendChild(fragment);
  }
}

function renderEncounterRow(entry) {
  const row = document.createElement("div");
  row.className = "encounter-row";

  const name = document.createElement("span");
  name.className = "encounter-name";
  name.textContent = entry.species_name || `#${entry.species_id}`;

  const levels = document.createElement("span");
  levels.className = "encounter-level";
  levels.textContent =
    entry.min_level === entry.max_level
      ? `Lv ${entry.min_level}`
      : `Lv ${entry.min_level}-${entry.max_level}`;

  const rate = document.createElement("span");
  rate.className = "encounter-slot-rate";
  rate.textContent = `${entry.slot_rate}%`;

  const catchRate = document.createElement("span");
  catchRate.className = "encounter-catch-rate";
  catchRate.textContent = `Catch ${entry.catch_rate}`;

  row.append(name, levels, rate, catchRate);
  return row;
}

function renderPartyCard(member) {
  const fragment = partyCardTemplate.content.cloneNode(true);
  const card = fragment.querySelector(".party-card");
  const slotPill = fragment.querySelector(".slot-pill");
  const art = fragment.querySelector(".pokemon-art");
  const fallback = fragment.querySelector(".sprite-fallback");
  const nickname = fragment.querySelector(".nickname");
  const level = fragment.querySelector(".level");
  const speciesPill = fragment.querySelector(".species-pill");
  const statusPill = fragment.querySelector(".status-pill");
  const hpLabel = fragment.querySelector(".hp-label");
  const hpValue = fragment.querySelector(".hp-value");
  const hpFill = fragment.querySelector(".hp-fill");
  const moveList = fragment.querySelector(".move-list");

  slotPill.textContent = `S${member.slot}`;
  nickname.textContent = member.nickname || "UNKNOWN";
  level.textContent = member.is_egg ? "EGG" : `Lv${member.level}`;
  speciesPill.textContent = `#${String(member.species_id).padStart(3, "0")}`;

  const hpRatio = member.max_hp > 0 ? member.current_hp / member.max_hp : 0;
  const fainted = !member.is_egg && member.max_hp > 0 && member.current_hp === 0;
  if (fainted) {
    card.classList.add("is-fainted");
  }

  const statusText = member.is_egg
    ? "egg"
    : fainted
      ? "fainted"
      : hpRatio <= 0.2
        ? "critical"
        : hpRatio <= 0.5
          ? "wounded"
          : "healthy";
  statusPill.textContent = statusText;
  if (fainted) {
    statusPill.classList.add("is-fainted");
  }

  hpLabel.textContent = "HP";
  hpValue.textContent = `${member.current_hp}/${member.max_hp}`;
  hpFill.style.width = `${Math.max(0, Math.min(100, hpRatio * 100))}%`;
  if (hpRatio <= 0.2) {
    hpFill.classList.add("is-low");
  } else if (hpRatio <= 0.5) {
    hpFill.classList.add("is-mid");
  }

  for (const moveId of member.moves || []) {
    if (!moveId) continue;
    const chip = document.createElement("span");
    chip.className = "move-chip";
    chip.textContent = moveNameFromId(moveId);
    moveList.appendChild(chip);
  }

  if (!moveList.children.length) {
    const chip = document.createElement("span");
    chip.className = "move-chip";
    chip.textContent = member.is_egg ? "No moves yet" : "No move data";
    moveList.appendChild(chip);
  }

  fallback.textContent = `#${String(member.species_id).padStart(3, "0")}`;
  art.alt = `${member.nickname || "Pokemon"} artwork`;
  art.src = `/sprites/${member.species_id}`;
  art.addEventListener("error", () => {
    if (art.dataset.remoteTried === "true") {
      art.classList.add("is-hidden");
      fallback.classList.add("is-visible");
      return;
    }
    art.dataset.remoteTried = "true";
    art.src = `${REMOTE_SPRITE_BASE}/${member.species_id}.png`;
  });
  art.addEventListener("load", () => {
    art.classList.remove("is-hidden");
    fallback.classList.remove("is-visible");
  });

  return fragment;
}

function renderEmpty(message) {
  partyGrid.innerHTML = "";
  renderBattlePanel(null);
  emptyState.classList.add("is-visible");
  emptyState.querySelector(".empty-copy").textContent = message;
}

function setStatus(stateName, label) {
  overlayStatus.dataset.state = stateName;
  overlayStatus.textContent = label;
}

function truthy(value) {
  return value === "1" || value === "true" || value === "yes";
}

function moveNameFromId(moveId) {
  const key = MOVE_KEYS_BY_ID[moveId];
  if (!key) {
    return `Move ${moveId}`;
  }
  return MOVE_NAME_OVERRIDES[key] || humanizeMoveKey(key);
}

function humanizeMoveKey(key) {
  return key
    .split("_")
    .map((part) => {
      if (/^\d+$/.test(part)) {
        return part;
      }
      return `${part.charAt(0)}${part.slice(1).toLowerCase()}`;
    })
    .join(" ");
}
