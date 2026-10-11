-- The Player window controller of silo-omarchy. It draws the seek bar with the
-- Marker ranges, play and pause, the audio and subtitle track names and the Skip
-- prompt in the theme colors. It reads nothing from disk: the app sends the theme
-- with "silo-theme" after this script sends "silo-ready", the Markers with
-- "silo-markers" and the prompt with "silo-prompt". The app decides when the
-- prompt shows. This script only draws it and sends "silo-action" when the user
-- clicks it or presses Enter.
local mp = require "mp"
local assdraw = require "mp.assdraw"

-- Tokyo Night, until the app sends the theme.
local theme = { accent = "7aa2f7", fg = "c0caf5", bg = "1a1b26" }
local visible_until = 0
local overlay = mp.create_osd_overlay("ass-events")
local seek_box -- the clickable seek bar of the last draw
local prompt_box -- the clickable Skip prompt of the last draw
local markers, prompt = {}, ""

-- "#RRGGBB" to the ASS color "BBGGRR".
local function bgr(hex)
  hex = hex:gsub("#", "")
  return hex:sub(5, 6) .. hex:sub(3, 4) .. hex:sub(1, 2)
end

local function clock(t)
  -- mpv uses Lua 5.1 or LuaJIT, which have no "//" operator.
  t = math.floor(t or 0)
  if t >= 3600 then
    return string.format("%d:%02d:%02d", math.floor(t / 3600), math.floor(t % 3600 / 60), t % 60)
  end
  return string.format("%d:%02d", math.floor(t / 60), t % 60)
end

local function track_name(kind)
  local track = mp.get_property_native("current-tracks/" .. kind)
  if not track then return "Off" end
  return track.title or track.lang or ("#" .. track.id)
end

local function rect(a, color, alpha, x0, y0, x1, y1)
  a:new_event()
  a:append(string.format("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H%s&\\1a&H%s&}", color, alpha))
  a:draw_start()
  a:rect_cw(x0, y0, x1, y1)
  a:draw_stop()
end

local function text(a, x, y, align, size, s)
  a:new_event()
  a:append(string.format("{\\an%d\\pos(%d,%d)\\fs%d\\bord0\\shad0\\1c&H%s&\\fnsans-serif}%s",
    align, x, y, size, bgr(theme.fg), s))
end

local function draw()
  local w, h = mp.get_osd_size()
  if not w or w == 0 then return end
  local a = assdraw.ass_new()
  seek_box, prompt_box = nil, nil
  local paused = mp.get_property_bool("pause")
  local bar = paused or mp.get_time() < visible_until
  if bar then
    local pos = mp.get_property_number("time-pos", 0)
    local dur = mp.get_property_number("duration", 0)
    local pad, band = 40, 110
    rect(a, bgr(theme.bg), "40", 0, h - band, w, h)
    local x0, x1, y = pad, w - pad, h - band + 30
    rect(a, bgr(theme.fg), "B0", x0, y, x1, y + 6)
    if dur > 0 then
      for _, m in ipairs(markers) do
        local s, e = math.max(m.s, 0) / dur, math.min(m.e, dur) / dur
        if e > s then rect(a, bgr(theme.fg), "60", x0 + (x1 - x0) * s, y, x0 + (x1 - x0) * e, y + 6) end
      end
      rect(a, bgr(theme.accent), "00", x0, y, x0 + (x1 - x0) * math.min(pos / dur, 1), y + 6)
      seek_box = { x0 = x0, y0 = y - 12, x1 = x1, y1 = y + 18, dur = dur }
    end
    text(a, x0, h - 30, 1, 28, (paused and "❚❚  " or "▶  ") .. clock(pos) .. " / " .. clock(dur)
      .. "    " .. mp.get_property("media-title", ""))
    text(a, x1, h - 30, 3, 24, "[a] Audio: " .. track_name("audio") .. "    [s] Subtitles: " .. track_name("sub"))
  end
  if prompt ~= "" then
    local px1, py1 = w - 40, h - (bar and 140 or 40)
    local px0, py0 = px1 - 260, py1 - 60
    rect(a, bgr(theme.bg), "20", px0, py0, px1, py1)
    rect(a, bgr(theme.accent), "00", px0, py1 - 4, px1, py1)
    text(a, (px0 + px1) / 2, (py0 + py1) / 2, 5, 28, prompt .. "  ⏎")
    prompt_box = { x0 = px0, y0 = py0, x1 = px1, y1 = py1 }
  end
  overlay.res_x, overlay.res_y, overlay.data = w, h, a.text
  overlay:update()
end

-- Shows the bar for 2.5 seconds.
local function poke()
  visible_until = mp.get_time() + 2.5
  draw()
end

mp.register_script_message("silo-theme", function(accent, fg, bg)
  theme = { accent = accent, fg = fg, bg = bg }
  draw()
end)

-- "kind:start:end,kind:start:end", in seconds of player time.
mp.register_script_message("silo-markers", function(list)
  markers = {}
  for kind, s, e in (list or ""):gmatch("(%a+):([%d.e+-]+):([%d.e+-]+)") do
    markers[#markers + 1] = { kind = kind, s = tonumber(s), e = tonumber(e) }
  end
  draw()
end)

-- An empty label hides the prompt.
mp.register_script_message("silo-prompt", function(label)
  prompt = label or ""
  draw()
end)

local function inside(b, x, y)
  return b and x >= b.x0 and x <= b.x1 and y >= b.y0 and y <= b.y1
end

-- Forced bindings win over the default keys. "s" replaces the screenshot key.
mp.add_forced_key_binding("ENTER", "silo-enter", function()
  if prompt ~= "" then mp.commandv("script-message", "silo-action") end
end)
mp.add_forced_key_binding("a", "silo-audio", function() mp.command("cycle aid"); poke() end)
mp.add_forced_key_binding("s", "silo-subtitles", function() mp.command("cycle sid"); poke() end)
mp.add_forced_key_binding("MBTN_LEFT", "silo-click", function()
  local x, y = mp.get_mouse_pos()
  local b = seek_box
  if inside(prompt_box, x, y) then
    mp.commandv("script-message", "silo-action")
  elseif inside(b, x, y) then
    mp.commandv("seek", tostring((x - b.x0) / (b.x1 - b.x0) * b.dur), "absolute")
  else
    mp.command("cycle pause")
  end
  poke()
end)

mp.observe_property("mouse-pos", "native", poke)
mp.observe_property("pause", "bool", draw)
mp.observe_property("osd-dimensions", "native", draw)
mp.register_event("seek", poke)
mp.add_periodic_timer(0.25, draw)

mp.commandv("script-message", "silo-ready")
