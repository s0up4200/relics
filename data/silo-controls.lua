-- The Player window controller of silo-omarchy. It draws the seek bar, play and
-- pause, and the audio and subtitle track names in the theme colors. It reads
-- nothing from disk: the app sends the theme with "silo-theme" after this script
-- sends "silo-ready".
local mp = require "mp"
local assdraw = require "mp.assdraw"

-- Tokyo Night, until the app sends the theme.
local theme = { accent = "7aa2f7", fg = "c0caf5", bg = "1a1b26" }
local visible_until = 0
local overlay = mp.create_osd_overlay("ass-events")
local seek_box -- the clickable seek bar of the last draw

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
  seek_box = nil
  local paused = mp.get_property_bool("pause")
  if paused or mp.get_time() < visible_until then
    local pos = mp.get_property_number("time-pos", 0)
    local dur = mp.get_property_number("duration", 0)
    local pad, band = 40, 110
    rect(a, bgr(theme.bg), "40", 0, h - band, w, h)
    local x0, x1, y = pad, w - pad, h - band + 30
    rect(a, bgr(theme.fg), "B0", x0, y, x1, y + 6)
    if dur > 0 then
      rect(a, bgr(theme.accent), "00", x0, y, x0 + (x1 - x0) * math.min(pos / dur, 1), y + 6)
      seek_box = { x0 = x0, y0 = y - 12, x1 = x1, y1 = y + 18, dur = dur }
    end
    text(a, x0, h - 30, 1, 28, (paused and "❚❚  " or "▶  ") .. clock(pos) .. " / " .. clock(dur)
      .. "    " .. mp.get_property("media-title", ""))
    text(a, x1, h - 30, 3, 24, "[a] Audio: " .. track_name("audio") .. "    [s] Subtitles: " .. track_name("sub"))
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

-- Forced bindings win over the default keys. "s" replaces the screenshot key.
mp.add_forced_key_binding("a", "silo-audio", function() mp.command("cycle aid"); poke() end)
mp.add_forced_key_binding("s", "silo-subtitles", function() mp.command("cycle sid"); poke() end)
mp.add_forced_key_binding("MBTN_LEFT", "silo-click", function()
  local x, y = mp.get_mouse_pos()
  local b = seek_box
  if b and x >= b.x0 and x <= b.x1 and y >= b.y0 and y <= b.y1 then
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
