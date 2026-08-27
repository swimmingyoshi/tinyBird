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

const OVERLAY_SECTIONS = new Set(["full", "party", "area", "battle", "generic"]);

const state = {
  lastOkAt: 0,
  currentSignature: "",
  section: "full",
  showEmpty: true,
  /**
   * True once a host page pushes us a snapshot.
   *
   * The overlay normally polls the desktop app's JSON export. Embedded in the
   * play page there is no desktop app — the emulator is right there in the
   * parent frame — so the parent posts snapshots instead and polling would
   * only overwrite them with stale data.
   */
  driven: false,
};

const overlayRoot = document.getElementById("overlay-root");
const partyPanel = document.getElementById("party-panel");
const partyGrid = document.getElementById("party-grid");
const partyCount = document.getElementById("party-count");
const emptyState = document.getElementById("empty-state");
const overlayTitle = document.getElementById("overlay-title");
const overlaySubtitle = document.getElementById("overlay-subtitle");
const overlayStatus = document.getElementById("overlay-status");
const battlePanel = document.getElementById("battle-panel");
const battleKicker = document.getElementById("battle-kicker");
const battleName = document.getElementById("battle-name");
const battleArt = document.getElementById("battle-art");
const battleFallback = document.getElementById("battle-fallback");
const battleLevel = document.getElementById("battle-level");
const battleHp = document.getElementById("battle-hp");
const battleCatch = document.getElementById("battle-catch");
const battleHpFill = document.getElementById("battle-hp-fill");
const battleProfile = document.getElementById("battle-profile");
const battleStats = document.getElementById("battle-stats");
const battleIvs = document.getElementById("battle-ivs");
const battleMoves = document.getElementById("battle-moves");
const areaPanel = document.getElementById("area-panel");
const areaName = document.getElementById("area-name");
const areaMapId = document.getElementById("area-map-id");
const encounterList = document.getElementById("encounter-list");
const genericPanel = document.getElementById("generic-panel");
const partyCardTemplate = document.getElementById("party-card-template");
const encounterMethodTemplate = document.getElementById("encounter-method-template");

initLayout();
listenForPushedSnapshots();
tick();
window.setInterval(tick, POLL_INTERVAL_MS);

/**
 * Accept snapshots pushed by a host page.
 *
 * Same schema as `/api/snapshot`, so both sources land in the same renderer and
 * the OBS pages and the in-page overlay cannot drift apart.
 */
function listenForPushedSnapshots() {
  if (window.parent === window) return;

  window.addEventListener("message", (event) => {
    // Same-origin only: the host is our own page, served from this server.
    if (event.origin !== window.location.origin) return;
    const message = event.data;
    if (!message || message.type !== "tinybird:snapshot") return;

    state.driven = true;
    state.lastOkAt = Date.now();

    const signature = JSON.stringify(message.snapshot);
    if (signature !== state.currentSignature) {
      state.currentSignature = signature;
      renderSnapshot(message.snapshot);
    }
    setStatus("live", message.status || "live");
  });

  // Tell the host we are ready for data.
  window.parent.postMessage({ type: "tinybird:overlay-ready" }, window.location.origin);
}

async function tick() {
  // A host feeding us directly is authoritative; do not poll over the top.
  if (state.driven) return;

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
      renderEmpty("Waiting for a local snapshot export.", state.showEmpty);
    }
  }
}

function initLayout() {
  const params = new URLSearchParams(window.location.search);
  const section = overlaySectionFromLocation(params);
  const singleSection = section !== "full";
  const transparent = params.has("transparent") ? truthy(params.get("transparent")) : singleSection;
  const compact = truthy(params.get("compact"));
  const layout = params.get("layout") || "column";
  const align = params.get("align") || "left";
  const showHeader = params.has("hideHeader") ? !truthy(params.get("hideHeader")) : !singleSection;
  const showLabels = params.has("hideLabels") ? !truthy(params.get("hideLabels")) : !singleSection;
  const showEmpty = params.has("showEmpty")
    ? truthy(params.get("showEmpty"))
    : section === "full" || section === "party";

  state.section = section;
  state.showEmpty = showEmpty;
  document.body.dataset.transparent = transparent ? "true" : "false";
  overlayRoot.dataset.transparent = transparent ? "true" : "false";
  overlayRoot.dataset.compact = compact ? "true" : "false";
  overlayRoot.dataset.section = section;
  overlayRoot.dataset.layout = ["column", "row", "stack"].includes(layout)
    ? layout
    : "column";
  overlayRoot.dataset.align = ["left", "center", "right"].includes(align) ? align : "left";
  overlayRoot.dataset.header = showHeader ? "visible" : "hidden";
  overlayRoot.dataset.labels = showLabels ? "visible" : "hidden";
}

function overlaySectionFromLocation(params) {
  const pathParts = window.location.pathname.split("/").filter(Boolean);
  const pathSection = pathParts[0] === "overlay" && pathParts[1] ? pathParts[1] : null;
  const section = params.get("section") || params.get("view") || pathSection || "full";
  return OVERLAY_SECTIONS.has(section) ? section : "full";
}

function renderSnapshot(snapshot) {
  const addon = snapshot?.addon;
  const fireRed = addon?.data?.type === "fire_red" ? addon.data.payload : null;
  const genericSections = Array.isArray(addon?.sections) ? addon.sections : [];
  const section = state.section;

  if (section === "generic" || (!fireRed && genericSections.length)) {
    renderGenericSnapshot(snapshot, genericSections);
    return;
  }

  if (!fireRed?.party?.length) {
    overlayTitle.textContent = addon?.display_name || snapshot?.rom?.title || "tinyBird Overlay";
    overlaySubtitle.textContent = fireRed
      ? "Supported addon found, but no live party members were exported yet."
      : genericSections.length
        ? "Generic addon sections are available."
        : "Load FireRed or LeafGreen in the desktop app to populate the party overlay.";
    if (genericSections.length) {
      renderGenericSnapshot(snapshot, genericSections);
    } else {
      renderEmpty("No party payload is available yet.", state.showEmpty);
    }
    return;
  }

  const rom = snapshot?.rom;
  overlayTitle.textContent = addon.display_name || "Party Overlay";
  overlaySubtitle.textContent = [rom?.title, rom?.game_code].filter(Boolean).join("  ");

  const showParty = section === "full" || section === "party";
  const showBattle = section === "full" || section === "battle";
  const showArea = section === "area" || (section === "full" && !fireRed.battle);

  emptyState.classList.remove("is-visible");
  renderGenericPanel([]);
  renderPartyPanel(showParty ? fireRed.party : []);
  renderBattlePanel(showBattle ? fireRed.battle : null);
  renderAreaPanel(showArea ? fireRed.area : null);

  if (section === "battle" && !fireRed.battle) {
    renderEmpty("No active battle right now.", state.showEmpty);
  } else if (section === "area" && !fireRed.area) {
    renderEmpty("Area data is pending.", state.showEmpty);
  }
}

function renderGenericSnapshot(snapshot, sections) {
  const addon = snapshot?.addon;
  const rom = snapshot?.rom;
  overlayTitle.textContent = addon?.display_name || rom?.title || "tinyBird Overlay";
  overlaySubtitle.textContent = [
    addon?.addon_id,
    addon?.version ? `v${addon.version}` : null,
    rom?.game_code,
  ]
    .filter(Boolean)
    .join("  ");

  renderPartyPanel([]);
  renderBattlePanel(null);
  renderAreaPanel(null);
  emptyState.classList.remove("is-visible");
  renderGenericPanel(sections);

  if (!sections.length) {
    renderEmpty("No generic addon sections are available yet.", state.showEmpty);
  }
}

function renderPartyPanel(party) {
  partyGrid.innerHTML = "";
  partyPanel.classList.toggle("is-visible", party.length > 0);
  partyPanel.hidden = party.length === 0;
  partyCount.textContent = party.length === 1 ? "1 live slot" : `${party.length} live slots`;

  for (const member of party) {
    partyGrid.appendChild(renderPartyCard(member));
  }
}

function renderBattlePanel(battle) {
  if (!battle?.opponent) {
    battlePanel.classList.remove("is-visible");
    battleArt.removeAttribute("src");
    battleArt.classList.add("is-hidden");
    battleFallback.classList.remove("is-visible");
    return;
  }

  const opponent = battle.opponent;
  const hpRatio = opponent.max_hp > 0 ? opponent.current_hp / opponent.max_hp : 0;
  battlePanel.classList.add("is-visible");
  battleKicker.textContent = `${battle.battle_kind || "Wild"} Battle`;
  battleName.textContent = opponent.species_name || `#${opponent.species_id}`;
  setPokemonSprite(
    battleArt,
    battleFallback,
    opponent.species_id,
    `${opponent.species_name || "Opponent Pokemon"} artwork`,
  );
  battleLevel.textContent = `Lv ${opponent.level}`;
  battleHp.textContent = `${opponent.current_hp}/${opponent.max_hp} HP`;
  battleCatch.textContent = battle.catchable
    ? `Catch ${opponent.catch_rate ?? "?"}`
    : "Not catchable";
  battleCatch.dataset.catchable = battle.catchable ? "true" : "false";
  battleHpFill.style.width = `${Math.max(0, Math.min(100, hpRatio * 100))}%`;
  battleHpFill.classList.toggle("is-low", hpRatio <= 0.2);
  battleHpFill.classList.toggle("is-mid", hpRatio > 0.2 && hpRatio <= 0.5);
  battleProfile.innerHTML = "";
  for (const value of [
    opponent.nature,
    opponent.ability_name,
    opponent.held_item?.name || "No item",
  ].filter(Boolean)) {
    battleProfile.appendChild(makeScoutPill(value));
  }
  renderStatList(battleStats, opponent.stats);
  battleIvs.textContent = formatSpread("IV", opponent.ivs, opponent.iv_total);
  battleMoves.textContent = formatMoves(opponent);
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

function renderGenericPanel(sections) {
  genericPanel.innerHTML = "";
  genericPanel.classList.toggle("is-visible", sections.length > 0);
  for (const section of sections) {
    genericPanel.appendChild(renderGenericSection(section));
  }
}

function renderGenericSection(section) {
  const article = document.createElement("article");
  article.className = "generic-section";

  const heading = document.createElement("div");
  heading.className = "generic-heading";
  const title = document.createElement("h2");
  title.textContent = section.title || section.section_id || "Addon Section";
  heading.append(title);
  // The addon's flag, if it raised one. On a stream this is the whole point of
  // the mechanism: a viewer who is not clicking through tabs still sees that
  // the thing on screen is worth catching.
  if (section.badge?.text) {
    const flag = document.createElement("strong");
    flag.className = "generic-flag";
    flag.textContent = section.badge.text;
    if (section.badge.tone) flag.dataset.tone = section.badge.tone;
    heading.append(flag);
  }
  // The section note, when it has one. This used to print the schema kind as a
  // fallback, which put the words "key_value" and "cards" on a stream overlay;
  // a section with nothing to say is better off saying nothing.
  if (section.note) {
    const note = document.createElement("p");
    note.textContent = section.note;
    heading.append(note);
  }
  article.appendChild(heading);

  if (section.kind === "key_value") {
    article.appendChild(renderGenericFields(section.payload || []));
  } else if (section.kind === "table") {
    article.appendChild(renderGenericTable(section.payload));
  } else if (section.kind === "cards") {
    article.appendChild(renderGenericCards(section.payload || []));
  } else if (section.kind === "list") {
    article.appendChild(renderGenericList(section.payload || []));
  } else {
    const fallback = document.createElement("pre");
    fallback.className = "generic-json";
    fallback.textContent = JSON.stringify(section.payload ?? section, null, 2);
    article.appendChild(fallback);
  }

  return article;
}

function renderGenericFields(fields) {
  const list = document.createElement("dl");
  list.className = "generic-fields";
  for (const field of fields) {
    const item = document.createElement("div");
    if (field.tone) item.dataset.tone = field.tone;
    const label = document.createElement("dt");
    const value = document.createElement("dd");
    label.textContent = field.label || "";
    value.textContent = field.value ?? "";
    item.append(label, value);
    if (field.meter) item.appendChild(renderGenericMeter(field.meter, field.tone));
    list.appendChild(item);
  }
  return list;
}

/** A bar, drawn from the numbers rather than by parsing the value string. */
function renderGenericMeter(meter, tone) {
  const max = Number(meter?.max) || 0;
  const value = Number(meter?.value) || 0;
  const bar = document.createElement("div");
  bar.className = "generic-meter";
  if (tone) bar.dataset.tone = tone;
  const fill = document.createElement("span");
  fill.style.width = `${max === 0 ? 0 : Math.min(100, Math.round((value / max) * 100))}%`;
  bar.appendChild(fill);
  return bar;
}

/**
 * Cards: a party, a squad, an opponent. Nothing is collapsed here — an overlay
 * is read at a glance from across a room and has no one to click it — so a
 * card shows its headline, its flags, and its detail all at once.
 */
function renderGenericCards(cards) {
  const wrap = document.createElement("div");
  wrap.className = "generic-cards";

  for (const card of cards) {
    const item = document.createElement("article");
    item.className = "generic-card";
    if (card.lead?.tone) item.dataset.tone = card.lead.tone;

    const head = document.createElement("header");
    // The picture, when the addon named one. An overlay is watched rather than
    // read, so a species sprite carries further than its name does.
    if (card.image?.src) {
      const img = document.createElement("img");
      img.className = "generic-portrait";
      img.src = card.image.src;
      img.alt = card.image.alt ?? "";
      img.loading = "lazy";
      img.addEventListener("error", () => img.remove());
      head.appendChild(img);
    }
    const title = document.createElement("h3");
    title.textContent = card.title || "";
    head.appendChild(title);
    if (card.subtitle) {
      const subtitle = document.createElement("p");
      subtitle.textContent = card.subtitle;
      head.appendChild(subtitle);
    }
    if (card.lead) {
      const lead = document.createElement("strong");
      lead.textContent = card.lead.value ?? "";
      head.appendChild(lead);
    }
    item.appendChild(head);

    if (card.lead?.meter) {
      item.appendChild(renderGenericMeter(card.lead.meter, card.lead.tone));
    }

    if (card.badges?.length) {
      const chips = document.createElement("div");
      chips.className = "generic-chips";
      for (const badge of card.badges) {
        const chip = document.createElement("span");
        chip.textContent = badge.text ?? "";
        if (badge.tone) chip.dataset.tone = badge.tone;
        chips.appendChild(chip);
      }
      item.appendChild(chips);
    }

    if (card.fields?.length) {
      item.appendChild(renderGenericFields(card.fields));
    }

    wrap.appendChild(item);
  }

  return wrap;
}

function renderGenericList(items) {
  const list = document.createElement("ul");
  list.className = "generic-list";
  for (const item of items) {
    const row = document.createElement("li");
    row.textContent = item;
    list.appendChild(row);
  }
  return list;
}

function renderGenericTable(table) {
  const wrap = document.createElement("div");
  wrap.className = "generic-table-wrap";
  const grid = document.createElement("div");
  grid.className = "generic-table";
  const columns = Array.isArray(table?.columns) ? table.columns : [];
  const rows = Array.isArray(table?.rows) ? table.rows : [];
  grid.style.setProperty("--columns", Math.max(1, columns.length));

  for (const column of columns) {
    const cell = document.createElement("strong");
    cell.textContent = column;
    grid.appendChild(cell);
  }
  for (const row of rows) {
    const values = Array.isArray(row) ? row : [row];
    for (let idx = 0; idx < Math.max(columns.length, values.length); idx += 1) {
      const cell = document.createElement("span");
      cell.textContent = values[idx] ?? "";
      grid.appendChild(cell);
    }
  }

  wrap.appendChild(grid);
  return wrap;
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
  const naturePill = fragment.querySelector(".nature-pill");
  const abilityPill = fragment.querySelector(".ability-pill");
  const hpLabel = fragment.querySelector(".hp-label");
  const hpValue = fragment.querySelector(".hp-value");
  const hpFill = fragment.querySelector(".hp-fill");
  const partyProfile = fragment.querySelector(".party-profile");
  const ivSpread = fragment.querySelector(".iv-spread");
  const evSpread = fragment.querySelector(".ev-spread");
  const moveList = fragment.querySelector(".move-list");

  slotPill.textContent = `S${member.slot}`;
  nickname.textContent = member.nickname || "UNKNOWN";
  level.textContent = member.is_egg ? "EGG" : `Lv${member.level}`;
  speciesPill.textContent = `#${String(member.species_id).padStart(3, "0")}`;
  naturePill.textContent = member.nature || "Nature ?";
  abilityPill.textContent = member.ability_name || "Ability ?";

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

  partyProfile.textContent = [
    member.species_name,
    member.held_item?.name ? `Item ${member.held_item.name}` : "No item",
  ]
    .filter(Boolean)
    .join("  ");
  ivSpread.textContent = formatSpread("IV", member.ivs, member.iv_total);
  evSpread.textContent = formatSpread("EV", member.evs, member.ev_total);

  const moveSlots = normalizedMoveSlots(member);
  for (const slot of moveSlots) {
    const chip = document.createElement("span");
    chip.className = "move-chip";
    chip.textContent = slot.pp == null ? slot.name : `${slot.name} ${slot.pp}PP`;
    moveList.appendChild(chip);
  }

  if (!moveList.children.length) {
    const chip = document.createElement("span");
    chip.className = "move-chip";
    chip.textContent = member.is_egg ? "No moves yet" : "No move data";
    moveList.appendChild(chip);
  }

  setPokemonSprite(
    art,
    fallback,
    member.species_id,
    `${member.nickname || member.species_name || "Pokemon"} artwork`,
  );

  return fragment;
}

function setPokemonSprite(art, fallback, speciesId, altText) {
  const speciesKey = String(speciesId || 0);
  const paddedSpecies = speciesKey.padStart(3, "0");
  fallback.textContent = `#${paddedSpecies}`;
  art.alt = altText;
  if (art.dataset.speciesId === speciesKey && art.getAttribute("src")) {
    return;
  }
  art.dataset.speciesId = speciesKey;
  art.dataset.remoteTried = "false";
  art.classList.remove("is-hidden");
  fallback.classList.remove("is-visible");
  art.onerror = () => {
    if (art.dataset.remoteTried === "true") {
      art.classList.add("is-hidden");
      fallback.classList.add("is-visible");
      return;
    }
    art.dataset.remoteTried = "true";
    art.src = `${REMOTE_SPRITE_BASE}/${speciesId}.png`;
  };
  art.onload = () => {
    art.classList.remove("is-hidden");
    fallback.classList.remove("is-visible");
  };
  art.src = `/sprites/${speciesId}`;
}

function renderEmpty(message, visible = true) {
  renderPartyPanel([]);
  renderBattlePanel(null);
  renderAreaPanel(null);
  renderGenericPanel([]);
  if (!visible) {
    emptyState.classList.remove("is-visible");
    return;
  }
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

function makeScoutPill(text) {
  const pill = document.createElement("span");
  pill.textContent = text;
  return pill;
}

function renderStatList(container, stats) {
  container.innerHTML = "";
  const rows = [
    ["HP", stats?.hp],
    ["Attack", stats?.attack],
    ["Defense", stats?.defense],
    ["Speed", stats?.speed],
    ["Sp. Atk", stats?.sp_attack],
    ["Sp. Def", stats?.sp_def],
  ];
  for (const [label, value] of rows) {
    const row = document.createElement("div");
    row.className = "battle-stat-row";
    const labelEl = document.createElement("span");
    labelEl.className = "battle-stat-label";
    labelEl.textContent = label;
    const valueEl = document.createElement("strong");
    valueEl.textContent = value ?? "?";
    row.append(labelEl, valueEl);
    container.appendChild(row);
  }
}

function formatSpread(label, spread, total) {
  if (!spread) {
    return `${label} pending`;
  }
  return `${label} ${spread.hp ?? "?"}/${spread.attack ?? "?"}/${spread.defense ?? "?"}/${
    spread.speed ?? "?"
  }/${spread.sp_attack ?? "?"}/${spread.sp_def ?? "?"} T${total ?? "?"}`;
}

function formatMoves(mon) {
  const slots = normalizedMoveSlots(mon);
  if (!slots.length) {
    return "Moves pending";
  }
  return slots.map((slot) => (slot.pp == null ? slot.name : `${slot.name} ${slot.pp}PP`)).join("  ");
}

function normalizedMoveSlots(mon) {
  if (Array.isArray(mon?.move_slots) && mon.move_slots.length) {
    return mon.move_slots.map((slot) => ({
      name: slot.name || moveNameFromId(slot.move_id),
      pp: slot.pp,
    }));
  }
  return (mon?.moves || [])
    .filter(Boolean)
    .map((moveId) => ({ name: moveNameFromId(moveId), pp: null }));
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
