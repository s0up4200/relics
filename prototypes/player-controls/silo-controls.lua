-- PROTOTYPE, throwaway. Custom player controls for silo-omarchy#8.
-- The app sends theme, markers and the skip prompt. This script only draws
-- them and sends "silo-action" back when the user takes the prompt.
local mp = require "mp"
local assdraw = require "mp.assdraw"

local theme = { accent = "BB9AF7", fg = "FFFFFF", bg = "000000" }
local markers, prompt = {}, ""
local visible_until = 0
local ov = mp.create_osd_overlay("ass-events")
local hit = {} -- clickable boxes from the last draw

local function bgr(hex) -- "#RRGGBB" to ASS "BBGGRR"
  hex = hex:gsub("#", "")
  return hex:sub(5, 6) .. hex:sub(3, 4) .. hex:sub(1, 2)
end

local function fmt(t)
  t = math.floor(t or 0)
  return string.format("%d:%02d", math.floor(t / 60), t % 60)
end

local function track_label(kind)
  local id = mp.get_property(kind == "audio" and "aid" or "sid")
  if not id or id == "no" then return "off" end
  for _, t in ipairs(mp.get_property_native("track-list") or {}) do
    if t.type == kind and tostring(t.id) == id then
      return t.title or t.lang or ("#" .. id)
    end
  end
  return "#" .. id
end

local function rect(a, color, alpha, x0, y0, x1, y1)
  a:new_event()
  a:append(string.format("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H%s&\\1a&H%s&}", color, alpha))
  a:draw_start()
  a:rect_cw(x0, y0, x1, y1)
  a:draw_stop()
end

local function text(a, x, y, align, size, color, s)
  a:new_event()
  a:append(string.format("{\\an%d\\pos(%d,%d)\\fs%d\\bord0\\shad0\\1c&H%s&\\fnsans-serif}%s",
    align, x, y, size, color, s))
end

local function draw()
  local w, h = mp.get_osd_size()
  if not w or w == 0 then return end
  local a = assdraw.ass_new()
  hit = {}
  local paused = mp.get_property_bool("pause")
  local show_bar = paused or mp.get_time() < visible_until

  if show_bar then
    local pos = mp.get_property_number("time-pos", 0)
    local dur = mp.get_property_number("duration", 1)
    local pad, bh = 40, 110
    rect(a, bgr(theme.bg), "40", 0, h - bh, w, h)
    local x0, x1, y = pad, w - pad, h - bh + 30
    rect(a, bgr(theme.fg), "B0", x0, y, x1, y + 6)
    for _, m in ipairs(markers) do
      rect(a, bgr(theme.fg), "60", x0 + (x1 - x0) * m.s / dur, y, x0 + (x1 - x0) * m.e / dur, y + 6)
    end
    rect(a, bgr(theme.accent), "00", x0, y, x0 + (x1 - x0) * pos / dur, y + 6)
    hit.seek = { x0, y - 12, x1, y + 18, dur = dur }
    local title = mp.get_property("media-title", "")
    text(a, x0, h - 30, 1, 28, bgr(theme.fg),
      (paused and "❚❚  " or "▶  ") .. fmt(pos) .. " / " .. fmt(dur) .. "    " .. title)
    text(a, x1, h - 30, 3, 24, bgr(theme.fg),
      "[a] Audio: " .. track_label("audio") .. "    [s] Subs: " .. track_label("sub"))
  end

  if prompt ~= "" then
    local bw, bh2 = 260, 60
    local px1, py1 = w - 40, h - (show_bar and 140 or 40)
    local px0, py0 = px1 - bw, py1 - bh2
    rect(a, bgr(theme.bg), "20", px0, py0, px1, py1)
    rect(a, bgr(theme.accent), "00", px0, py1 - 4, px1, py1)
    text(a, (px0 + px1) / 2, (py0 + py1) / 2, 5, 28, bgr(theme.fg), prompt .. "  ⏎")
    hit.prompt = { px0, py0, px1, py1 }
  end

  ov.res_x, ov.res_y, ov.data = w, h, a.text
  ov:update()
end

local function poke()
  visible_until = mp.get_time() + 2.5
  draw()
end

local function inside(b, x, y)
  return b and x >= b[1] and x <= b[3] and y >= b[2] and y <= b[4]
end

mp.register_script_message("silo-theme", function(acc, fg, bg)
  theme = { accent = acc, fg = fg, bg = bg }
  draw()
end)
mp.register_script_message("silo-markers", function(list)
  markers = {}
  for kind, s, e in list:gmatch("(%a+):([%d.]+):([%d.]+)") do
    markers[#markers + 1] = { kind = kind, s = tonumber(s), e = tonumber(e) }
  end
  draw()
end)
mp.register_script_message("silo-prompt", function(label)
  prompt = label or ""
  draw()
end)

mp.add_forced_key_binding("ENTER", "silo-enter", function()
  if prompt ~= "" then mp.commandv("script-message", "silo-action") end
end)
mp.add_forced_key_binding("a", "silo-audio", function() mp.command("cycle aid"); poke() end)
mp.add_forced_key_binding("s", "silo-subs", function() mp.command("cycle sid"); poke() end)
mp.add_forced_key_binding("MBTN_LEFT", "silo-click", function()
  local x, y = mp.get_mouse_pos()
  if inside(hit.prompt, x, y) then
    mp.commandv("script-message", "silo-action")
  elseif inside(hit.seek, x, y) then
    local frac = (x - hit.seek[1]) / (hit.seek[3] - hit.seek[1])
    mp.commandv("seek", tostring(frac * hit.seek.dur), "absolute")
  else
    mp.command("cycle pause")
  end
  poke()
end)

mp.observe_property("mouse-pos", "native", function() poke() end)
mp.observe_property("pause", "bool", draw)
mp.observe_property("osd-dimensions", "native", draw)
mp.register_event("seek", poke)
mp.add_periodic_timer(0.25, draw)

mp.commandv("script-message", "silo-ready")
