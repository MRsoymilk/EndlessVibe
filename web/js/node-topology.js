// Interactive Nodes topology. The animated paths explain protocol direction;
// they intentionally do not claim to show live per-peer traffic or connectivity.
const SVG_NS = "http://www.w3.org/2000/svg";
const CARD_WIDTH = 250;
const CARD_HEIGHT = 116;
const START_Y = 90;
const ROW_GAP = 166;
const LEFT_X = 78;
const RIGHT_X = 730;
const CENTER_X = 405;
const MIN_ZOOM = 0.22;
const MAX_ZOOM = 2.25;
// Show completed calls briefly; cap stale "running" calls if the service stops reporting updates.
const FLOW_FINISHED_TTL_SECONDS = 15;
const FLOW_RUNNING_TTL_SECONDS = 90;

let view = null;

function element(tag, className, text) {
  const value = document.createElement(tag);
  if (className) value.className = className;
  if (text !== undefined) value.textContent = String(text);
  return value;
}
function svgElement(tag, attributes = {}) {
  const value = document.createElementNS(SVG_NS, tag);
  for (const [name, attr] of Object.entries(attributes)) value.setAttribute(name, String(attr));
  return value;
}
function safeString(value, fallback = "") {
  return typeof value === "string" && value.trim() ? value : fallback;
}
function remoteRole(peer) {
  // A stored peer.role describes THIS node's side of the relationship.
  return peer.role === "parent" ? "child" : peer.role === "child" ? "parent" : "unknown";
}
function roleTitle(role) {
  return {parent: "父节点 · Parent", child: "子节点 · Child",
    mixed: "角色冲突 · Mixed", unassigned: "尚未指定角色",
    unknown: "局域网节点"}[role] || "局域网节点";
}

export function deriveNodeTopology(discovery = {}, pending = [], trusted = []) {
  const localId = safeString(discovery.node_id, "local");
  const localRole = safeString(discovery.local_role, "unassigned");
  const local = {
    key: "local", id: localId, kind: "local", role: localRole,
    name: safeString(discovery.name, "EndlessVibe"), endpoint: safeString(discovery.listen),
    state: discovery.enabled ? "ready" : "disabled",
    status: discovery.enabled ? "本机 · Transfer 已启用" : "本机 · Transfer 未启用",
    note: "本机运行的 Transfer 服务", grants: []
  };
  const remote = new Map();
  for (const peer of Array.isArray(trusted) ? trusted : []) {
    const id = safeString(peer?.node_id);
    if (!id || id === localId || remote.has(id)) continue;
    const role = remoteRole(peer);
    const grants = Array.isArray(peer.grants) ? peer.grants : [];
    const connectivity=peer.role==="parent" ?
      safeString(peer.connection_state,"unknown") : "unknown";
    const healthLabel={
      online:"在线 · TLS 已验证",
      unreachable:"连接不可达 · 已配对",
      checking:"连接检测中 · 已配对",
      unsupported:"旧版节点 · 无法验证",
      disabled:"Transfer 已禁用",
      unknown:"连接状态未知 · 已配对"
    };
    remote.set(id, {
      key: id, id, kind: "trusted", role,
      name: safeString(peer.name, "已信任节点"), endpoint: safeString(peer.endpoint),
      state: "trusted", connectivity,
      lastVerified:peer.last_verified||null,
      latencyMs:peer.probe_latency_ms??null,
      status:role==="child" ? healthLabel[connectivity]||healthLabel.unknown :
        "PINNED TLS · 已配对",
      // Absence of multicast discovery does not establish that TLS is offline.
      note: connectivity==="unreachable" ? "TLS 探测失败 · 可能已关闭或网络中断" :
        connectivity==="checking" ? "正在确认节点连接" :
        connectivity==="unsupported" ? "子节点版本过旧 · 不支持 TLS 健康探测" :
        role === "parent" ?
          (grants.length ? "本机已授予 " + grants.length + " 个 Project" : "本机尚未授予 Project") :
          "Project 访问仍由子节点授权控制",
      discovery: peer.discovery_online === true ? "最近收到 LAN 广播" : "近期未发现广播 · 连通性未知",
      grants
    });
  }
  for (const peer of Array.isArray(pending) ? pending : []) {
    const id = safeString(peer?.node_id);
    if (!id || id === localId || remote.has(id)) continue;
    const rejected = peer.state === "rejected";
    remote.set(id, {
      key: id, id, kind: "pending", role: remoteRole(peer),
      name: safeString(peer.name, "等待配对的节点"), endpoint: safeString(peer.endpoint),
      state: rejected ? "rejected" : "pending",
      status: rejected ? "配对已拒绝" : "等待子节点确认",
      note: rejected ? "未建立可信连接；可重新发起配对" : "仅配对申请 · 尚无 Project 授权",
      grants: []
    });
  }
  for (const peer of Array.isArray(discovery.peers) ? discovery.peers : []) {
    const id = safeString(peer?.node_id);
    if (!id || id === localId || remote.has(id)) continue;
    remote.set(id, {
      key: id, id, kind: "discovered", role: "unknown",
      name: safeString(peer.name, "已发现节点"), endpoint: safeString(peer.address),
      state: peer.online === true ? "seen" : "stale",
      status: peer.online === true ? "LAN 广播 · 未配对" : "广播已过期 · 未配对",
      note: "仅通过 LAN 广播发现，没有可信操作通道", grants: []
    });
  }
  const peers = [...remote.values()].sort((a, b) =>
    ({trusted: 0, pending: 1, discovered: 2}[a.kind] -
     {trusted: 0, pending: 1, discovered: 2}[b.kind]) ||
    a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
  const edges = [];
  for (const peer of peers) {
    if (peer.kind === "trusted" || peer.kind === "pending") {
      const parent = peer.role === "parent" ? peer.key : "local";
      const child = peer.role === "parent" ? "local" : peer.key;
      if (peer.kind === "trusted") {
        edges.push({key: peer.key + ":request", peerKey: peer.key,
          from: parent, to: child, kind: "request", label: "操作请求"});
        edges.push({key: peer.key + ":response", peerKey: peer.key,
          from: child, to: parent, kind: "response", label: "结果返回"});
      } else {
        edges.push({key: peer.key + ":pairing", peerKey: peer.key,
          from: parent, to: child,
          kind: peer.state === "rejected" ? "rejected" : "pairing",
          label: peer.state === "rejected" ? "配对已拒绝" : "配对申请"});
      }
    }
  }
  return {
    nodes: [local, ...peers], edges, localRole,
    counts: {
      trusted: peers.filter(p => p.kind === "trusted").length,
      unreachable: peers.filter(p => p.kind === "trusted" && p.connectivity === "unreachable").length,
      online: peers.filter(p => p.kind === "trusted" && p.connectivity === "online").length,
      unsupported: peers.filter(p => p.kind === "trusted" && p.connectivity === "unsupported").length,
      pending: peers.filter(p => p.kind === "pending" && p.state !== "rejected").length,
      discovered: peers.filter(p => p.kind === "discovered").length
    }
  };
}

export function layoutNodeTopology(model) {
  const positions = new Map();
  const connected = model.nodes.filter(n => n.kind === "trusted" || n.kind === "pending");
  const parents = connected.filter(n => n.role === "parent");
  const children = connected.filter(n => n.role !== "parent");
  const span = Math.max(1, parents.length, children.length);
  const localX = model.localRole === "child" ? RIGHT_X :
    model.localRole === "mixed" ? CENTER_X : LEFT_X;
  positions.set("local", {x: localX, y: START_Y + (span - 1) * ROW_GAP / 2});
  for (const [index, item] of parents.entries()) {
    positions.set(item.key, {x: LEFT_X, y: START_Y + index * ROW_GAP});
  }
  for (const [index, item] of children.entries()) {
    positions.set(item.key, {x: RIGHT_X, y: START_Y + index * ROW_GAP});
  }
  // Untrusted discoveries are physically separate: no line implies a TLS link.
  const discovered = model.nodes.filter(n => n.kind === "discovered");
  const discoveredY = START_Y + span * ROW_GAP + 88;
  discovered.forEach((item, index) => positions.set(item.key, {
    x: LEFT_X + (index % 3) * 325,
    y: discoveredY + Math.floor(index / 3) * ROW_GAP
  }));
  return {positions, discoveredY: discovered.length ? discoveredY : null};
}


// Each node displays only local operation_log rows that explicitly identify
// its peer. Cross-machine sequence numbers must never be treated as a shared ID.
export function matchNodeFlowEvents(model, response, maxPerPeer = 6) {
  const byPeer = new Map();
  const trusted = new Map(model.nodes.filter(n => n.kind === "trusted")
    .map(n => [n.id, n]));
  if (response?.scope !== "local_only" || !Array.isArray(response.events)) return byPeer;
  const ordered = response.events.filter(e => Number.isSafeInteger(e?.seq) && e.seq > 0)
    .sort((a, b) => b.seq - a.seq);
  for (const event of ordered) {
    const peer = trusted.get(event.peer_id);
    if (!peer || !["parent","child"].includes(peer.role) ||
        (peer.role === "child" ? "outgoing" : "incoming") !== event.side)
      continue;
    const items = byPeer.get(peer.id) || [];
    if (items.length >= maxPerPeer || items.some(existing => existing.seq === event.seq))
      continue;
    items.push(event);
    byPeer.set(peer.id, items);
  }
  return byPeer;
}

function eventLabel(edge, event) {
  const seq = "#" + event.seq;
  const tool = safeString(event.tool, "unknown");
  if (edge.kind === "request") {
    const target = [safeString(event.workspace), safeString(event.project)].filter(Boolean).join("/");
    return seq + " · " + tool + (target ? " · " + target : "");
  }
  if (event.status === "running") return seq + " · 等待结果";
  const summary = safeString(event.result_summary, "状态未知");
  const duration = Number.isFinite(event.duration_ms) && event.duration_ms >= 0 ?
    " · " + event.duration_ms + " ms" : "";
  return seq + " · " + summary + duration;
}
function shortEventText(text, max = 39) {
  const value = String(text);
  return value.length > max ? value.slice(0, max - 1) + "…" : value;
}
// A newly observed call supersedes the previous call even if the previous
// result completed more recently. Expired calls must never reappear from history.
// Use the server's generated_at clock for timestamps; browser clocks can differ.
export function flowEventExpiresAt(event) {
  if (!event || !Number.isFinite(event.started) || event.started <= 0) return 0;
  if (event.status === "running") return event.started + FLOW_RUNNING_TTL_SECONDS;
  const finished = Number.isFinite(event.finished) && event.finished > 0 ?
    event.finished : event.started;
  return finished + FLOW_FINISHED_TTL_SECONDS;
}
export function activeNodeFlowEvent(events, serverNowSeconds) {
  if (!Array.isArray(events) || !events.length ||
      !Number.isFinite(serverNowSeconds)) return null;
  const latest = events[0];
  return flowEventExpiresAt(latest) > serverNowSeconds ? latest : null;
}
function renderFlowLabels() {
  if (!view?.model) return;
  if (view.flowExpiryTimer !== null) {
    clearTimeout(view.flowExpiryTimer);
    view.flowExpiryTimer = null;
  }
  const clock = view.flowClock;
  const nowSeconds = clock ?
    clock.serverSeconds + (performance.now() - clock.tick) / 1000 : Date.now() / 1000;
  let nextExpiry = Infinity;
  for (const edge of view.model.edges) {
    const parts = view.paths.get(edge.key);
    if (!parts?.event) continue;
    const linkPeer=view.model.nodes.find(n => n.key===edge.peerKey);
    const latest = view.flowAvailable && linkPeer?.connectivity!=="unreachable" ?
      activeNodeFlowEvent(view.flows.get(edge.peerKey), nowSeconds) : null;
    const text = latest ? eventLabel(edge, latest) : "";
    setText(parts.event, shortEventText(text));
    if (latest) {
      parts.event.dataset.state = latest.status || "unknown";
      parts.event.setAttribute("aria-label", text);
      nextExpiry = Math.min(nextExpiry, flowEventExpiresAt(latest));
    } else {
      parts.event.removeAttribute("data-state");
      parts.event.removeAttribute("aria-label");
    }
    const title = parts.event.querySelector("title");
    if (title) title.textContent = text;
  }
  // Expire even when polling pauses, the tab is hidden or no new calls arrive.
  if (Number.isFinite(nextExpiry)) {
    const delay = Math.max(25, Math.ceil((nextExpiry - nowSeconds) * 1000) + 25);
    view.flowExpiryTimer = setTimeout(renderFlowLabels, delay);
  }
}

function setText(target, value) {
  if (target && target.textContent !== String(value)) target.textContent = String(value);
}

function createCard(item) {
  const card = element("button", "node-topology-card");
  card.type = "button";
  card.dataset.nodeKey = item.key;
  const head = element("span", "node-topology-card-head");
  head.append(element("span", "node-topology-card-role"),
    element("span", "node-topology-card-status"));
  card.append(head, element("strong", "node-topology-card-name"),
    element("span", "node-topology-card-id"),
    element("span", "node-topology-card-note"));
  card.addEventListener("click", () => selectNode(item.key));
  return card;
}

function addMarker(defs, kind) {
  const marker = svgElement("marker", {
    id: "node-topology-arrow-" + kind,
    viewBox: "0 0 10 10", refX: 9, refY: 5,
    markerWidth: 7, markerHeight: 7, orient: "auto",
    markerUnits: "userSpaceOnUse"
  });
  const arrow = svgElement("path", {d: "M 0 0 L 10 5 L 0 10 Z"});
  arrow.setAttribute("class", "node-topology-arrow node-topology-arrow--" + kind);
  marker.append(arrow);
  defs.append(marker);
}

function showDetail(item) {
  if (!view) return;
  const host = view.detail;
  host.replaceChildren();
  if (!item) return;
  const head = element("div", "node-topology-detail-main");
  head.append(element("strong", "", item.name),
    element("span", "node-topology-detail-badge", item.status));
  const summary = element("div", "node-topology-detail-meta");
  summary.append(
    element("span", "", roleTitle(item.role)),
    element("span", "", "Node ID: " + item.id),
    element("span", "", "Endpoint: " + (item.endpoint || "未配置"))
  );
  const note = element("p", "", item.note);
  host.append(head, summary, note);
  if (item.discovery) host.append(element("p", "node-topology-detail-muted", item.discovery));
  if (item.kind === "trusted" && item.connectivity === "unreachable") {
    host.append(element("p", "node-topology-health-warning",
      "节点当前不可达：请检查子节点 EndlessVibe 服务、网络和 Transfer 端口。配对及授权仍保留。"));
  }
  if (item.kind === "trusted" && item.role === "child" && item.lastVerified) {
    host.append(element("p", "node-topology-detail-muted",
      "最近 TLS 验证成功：" + new Date(item.lastVerified*1000).toLocaleString()));
  }
  if (item.kind === "trusted" && item.role === "parent" && item.grants.length) {
    host.append(element("p", "node-topology-detail-muted",
      "授权 Project: " + item.grants.map(g => g.workspace + "/" + g.project).join(", ")));
  }
  if (item.kind === "trusted") {
    const recent = element("section", "node-topology-recent");
    const heading = element("div", "node-topology-recent-heading");
    heading.append(element("strong", "", "最近的操作请求 / 返回 · 本机日志"));
    const all = element("a", "", "操作日志 ↗");
    all.href = "/operations";
    heading.append(all);
    recent.append(heading);
    const entries = view.flows.get(item.id) || [];
    if (!view.flowAvailable) {
      recent.append(element("p", "node-topology-detail-muted", "操作记录暂不可用"));
    } else if (!entries.length) {
      recent.append(element("p", "node-topology-detail-muted", "暂无该节点的近期操作记录"));
    } else {
      const list = element("div", "node-topology-recent-list");
      for (const event of entries.slice(0, 5)) {
        const row = element("div", "node-topology-recent-row");
        const link = element("a", "node-topology-operation-link",
          "#" + event.seq + " · " + event.tool);
        link.href = "/operations?seq=" + encodeURIComponent(event.seq);
        const target = [event.workspace, event.project].filter(Boolean).join("/");
        const info = element("span", "node-topology-operation-target", target || "节点级操作");
        const result = element("span", "node-topology-operation-status",
          safeString(event.result_summary, "等待结果") +
          (event.status === "running" ? "" : " · " + event.duration_ms + " ms"));
        result.dataset.state = event.status;
        const when = Number.isFinite(event.started) && event.started > 0 ?
          new Date(event.started * 1000).toLocaleTimeString() : "时间未知";
        row.title = when + " · " + event.side + " · " + (target || "节点级操作");
        row.append(link, info, result);
        list.append(row);
      }
      recent.append(list);
    }
    host.append(recent);
  }
  if (item.kind !== "local") {
    const targetId = item.kind === "trusted" ? "node-trusted-list" :
      item.kind === "pending" ? "node-pending-list" : "node-discovered-list";
    const link = element("button", "small-button node-topology-jump", "查看下方节点详情 ↓");
    link.type = "button";
    link.addEventListener("click", () => {
      const list = document.getElementById(targetId);
      const card = [...(list?.querySelectorAll(".node-card") || [])]
        .find(candidate => candidate.dataset.nodeId === item.id);
      (card || list)?.scrollIntoView({behavior: "smooth", block: "center"});
    });
    host.append(link);
  }
}

function selectNode(key) {
  if (!view) return;
  if (!view.model.nodes.some(item => item.key === key)) return;
  view.selected = key;
  for (const [id, card] of view.cards) {
    const selected = id === key;
    card.classList.toggle("is-selected", selected);
    card.setAttribute("aria-pressed", String(selected));
  }
  for (const [edgeKey, parts] of view.paths) {
    const edge = view.model.edges.find(e => e.key === edgeKey);
    parts.path.classList.toggle("is-dimmed", key !== "local" && edge?.peerKey !== key);
    parts.label.classList.toggle("is-dimmed", key !== "local" && edge?.peerKey !== key);
    parts.event?.classList.toggle("is-dimmed", key !== "local" && edge?.peerKey !== key);
  }
  showDetail(view.model.nodes.find(item => item.key === key));
}

function connectionPath(edge) {
  const a = view.positions.get(edge.from);
  const b = view.positions.get(edge.to);
  if (!a || !b) return {path: "", x: 0, y: 0};
  const right = a.x <= b.x;
  const sign = right ? 1 : -1;
  const x1 = a.x + (right ? CARD_WIDTH : 0);
  const x2 = b.x + (right ? 0 : CARD_WIDTH);
  const separation = edge.kind === "request" ? -18 :
    edge.kind === "response" ? 18 : 0;
  const y1 = a.y + CARD_HEIGHT / 2 + separation;
  const y2 = b.y + CARD_HEIGHT / 2 + separation;
  const bend = Math.max(70, Math.abs(x2 - x1) * 0.46);
  return {
    path: "M " + x1 + " " + y1 + " C " + (x1 + sign * bend) + " " + y1 +
      " " + (x2 - sign * bend) + " " + y2 + " " + x2 + " " + y2,
    x: (x1 + x2) / 2,
    y: (y1 + y2) / 2 + (edge.kind === "request" ? -10 : 13)
  };
}

function drawGeometry() {
  if (!view?.model) return;
  let width = 1070;
  let height = 375;
  for (const [key, pos] of view.positions) {
    const card = view.cards.get(key);
    if (card) {
      card.style.left = pos.x + "px";
      card.style.top = pos.y + "px";
    }
    width = Math.max(width, pos.x + CARD_WIDTH + 90);
    height = Math.max(height, pos.y + CARD_HEIGHT + 95);
  }
  view.scene.style.width = width + "px";
  view.scene.style.height = height + "px";
  view.svg.setAttribute("viewBox", "0 0 " + width + " " + height);
  view.svg.setAttribute("width", width);
  view.svg.setAttribute("height", height);
  if (view.sectionLabel) {
    const label = view.sectionLabel;
    label.style.top = (view.discoveredY - 55) + "px";
    label.style.left = LEFT_X + "px";
  }
  for (const edge of view.model.edges) {
    const part = view.paths.get(edge.key);
    if (!part) continue;
    const info = connectionPath(edge);
    part.path.setAttribute("d", info.path);
    part.label.setAttribute("x", info.x);
    part.label.setAttribute("y", info.y);
    if (part.event) {
      part.event.setAttribute("x", info.x);
      part.event.setAttribute("y", info.y + (edge.kind === "request" ? -18 : 19));
    }
  }
}

function updateTransform() {
  if (!view) return;
  view.scene.style.transform =
    "translate(" + view.x + "px, " + view.y + "px) scale(" + view.scale + ")";
  setText(view.zoomLabel, Math.round(view.scale * 100) + "%");
}

function fitTopology() {
  if (!view?.positions.size) return;
  const points = [...view.positions.values()];
  const minX = Math.min(...points.map(p => p.x)) - 50;
  const maxX = Math.max(...points.map(p => p.x + CARD_WIDTH)) + 50;
  const minY = Math.min(...points.map(p => p.y)) - 65;
  const maxY = Math.max(...points.map(p => p.y + CARD_HEIGHT)) + 65;
  const rect = view.canvas.getBoundingClientRect();
  if (!rect.width || !rect.height) return;
  view.scale = Math.max(MIN_ZOOM, Math.min(1.12,
    (rect.width - 28) / (maxX - minX), (rect.height - 25) / (maxY - minY)));
  view.x = (rect.width - (maxX - minX) * view.scale) / 2 - minX * view.scale;
  view.y = (rect.height - (maxY - minY) * view.scale) / 2 - minY * view.scale;
  view.userMoved = false;
  updateTransform();
}

function zoomAt(factor, clientX, clientY) {
  if (!view) return;
  const next = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, view.scale * factor));
  const rect = view.canvas.getBoundingClientRect();
  const px = clientX - rect.left;
  const py = clientY - rect.top;
  const graphX = (px - view.x) / view.scale;
  const graphY = (py - view.y) / view.scale;
  view.x = px - graphX * next;
  view.y = py - graphY * next;
  view.scale = next;
  view.userMoved = true;
  updateTransform();
}

function centerZoom(factor) {
  const rect = view.canvas.getBoundingClientRect();
  zoomAt(factor, rect.left + rect.width / 2, rect.top + rect.height / 2);
}

function onPointerDown(event) {
  if (!view || event.button !== 0 || view.drag) return;
  const card = event.target.closest(".node-topology-card");
  const key = card?.dataset.nodeKey;
  view.drag = {
    pointer: event.pointerId, key: key || null,
    startX: event.clientX, startY: event.clientY,
    originX: view.x, originY: view.y,
    nodePos: key ? {...view.positions.get(key)} : null
  };
  if (card) card.focus({preventScroll: true});
  event.currentTarget.setPointerCapture(event.pointerId);
  event.preventDefault();
}

function onPointerMove(event) {
  if (!view?.drag || view.drag.pointer !== event.pointerId) return;
  const drag = view.drag;
  const dx = event.clientX - drag.startX;
  const dy = event.clientY - drag.startY;
  if (drag.key && drag.nodePos) {
    view.positions.set(drag.key, {
      x: drag.nodePos.x + dx / view.scale,
      y: drag.nodePos.y + dy / view.scale
    });
    if (Math.hypot(dx, dy) > 4) view.manual.add(drag.key);
    drawGeometry();
  } else {
    view.x = drag.originX + dx;
    view.y = drag.originY + dy;
    updateTransform();
  }
  if (Math.hypot(dx, dy) > 4) view.userMoved = true;
}

function onPointerUp(event) {
  if (!view?.drag || view.drag.pointer !== event.pointerId) return;
  const key = view.drag.key;
  const moved = Math.hypot(event.clientX - view.drag.startX,
    event.clientY - view.drag.startY) > 4;
  view.drag = null;
  if (event.currentTarget.hasPointerCapture(event.pointerId)) {
    event.currentTarget.releasePointerCapture(event.pointerId);
  }
  if (key && !moved) selectNode(key);
}

function onCanvasKey(event) {
  if (event.target !== view.canvas) return;
  const step = 48;
  if (event.key === "0" || event.key.toLowerCase() === "f") fitTopology();
  else if (event.key === "+" || event.key === "=") centerZoom(1.18);
  else if (event.key === "-" || event.key === "_") centerZoom(1 / 1.18);
  else if (event.key === "ArrowLeft") view.x += step;
  else if (event.key === "ArrowRight") view.x -= step;
  else if (event.key === "ArrowUp") view.y += step;
  else if (event.key === "ArrowDown") view.y -= step;
  else return;
  event.preventDefault();
  if (event.key.startsWith("Arrow")) {
    view.userMoved = true;
    updateTransform();
  }
}

export function mountNodeTopology() {
  if (view) return;
  const canvas = document.getElementById("node-topology-canvas");
  if (!canvas) return;
  const scene = element("div", "node-topology-scene");
  const svg = svgElement("svg", {class: "node-topology-svg", "aria-hidden": "true"});
  const defs = svgElement("defs");
  for (const kind of ["request", "response", "pairing", "rejected"]) addMarker(defs, kind);
  svg.append(defs);
  const cardsLayer = element("div", "node-topology-cards");
  scene.append(svg, cardsLayer);
  canvas.append(scene);
  view = {
    canvas, scene, svg, cardsLayer, paths: new Map(), cards: new Map(),
    positions: new Map(), manual: new Set(), model: null, selected: "local",
    flowAvailable: false, flows: new Map(), flowClock: null, flowExpiryTimer: null,
    x: 0, y: 0, scale: 1, drag: null, userMoved: false, discoveredY: null,
    sectionLabel: null, zoomLabel: document.getElementById("node-topology-zoom"),
    detail: document.getElementById("node-topology-detail")
  };
  document.getElementById("node-topology-fit")?.addEventListener("click", fitTopology);
  document.getElementById("node-topology-zoom-in")?.addEventListener("click", () => centerZoom(1.2));
  document.getElementById("node-topology-zoom-out")?.addEventListener("click", () => centerZoom(1 / 1.2));
  canvas.addEventListener("wheel", event => {
    event.preventDefault();
    zoomAt(Math.exp(-event.deltaY * 0.0014), event.clientX, event.clientY);
  }, {passive: false});
  canvas.addEventListener("pointerdown", onPointerDown);
  canvas.addEventListener("pointermove", onPointerMove);
  canvas.addEventListener("pointerup", onPointerUp);
  canvas.addEventListener("pointercancel", onPointerUp);
  canvas.addEventListener("keydown", onCanvasKey);
  window.addEventListener("resize", () => {if (view && !view.userMoved) fitTopology();});
  document.addEventListener("visibilitychange", () => {if (!document.hidden) renderFlowLabels();});
}

export function updateNodeTopology(discovery, pending, peers, flowResponse = null) {
  if (!view) return;
  const model = deriveNodeTopology(discovery, pending, peers);
  const previousIds = [...view.cards.keys()].sort().join("\u0000");
  const nextIds = model.nodes.map(n => n.key).sort().join("\u0000");
  const layout = layoutNodeTopology(model);
  const oldPositions = view.positions;
  const newPositions = new Map();
  for (const [key, pos] of layout.positions) {
    newPositions.set(key, view.manual.has(key) && oldPositions.has(key)
      ? oldPositions.get(key) : pos);
  }
  view.positions = newPositions;
  view.model = model;
  view.flowAvailable = flowResponse?.scope === "local_only" && Array.isArray(flowResponse.events);
  const serverSeconds = Number(flowResponse?.generated_at);
  view.flowClock = view.flowAvailable && Number.isFinite(serverSeconds) && serverSeconds > 0 ?
    {serverSeconds, tick: performance.now()} : null;
  view.flows = matchNodeFlowEvents(model, flowResponse);
  view.discoveredY = layout.discoveredY;

  const nextKeys = new Set(model.nodes.map(n => n.key));
  for (const [key, card] of view.cards) {
    if (!nextKeys.has(key)) {card.remove(); view.cards.delete(key); view.manual.delete(key);}
  }
  for (const item of model.nodes) {
    let card = view.cards.get(item.key);
    if (!card) {
      card = createCard(item);
      view.cards.set(item.key, card);
      view.cardsLayer.append(card);
    }
    card.dataset.kind = item.kind;
    card.dataset.state = item.state;
    card.dataset.connectivity = item.connectivity||"unknown";
    card.classList.toggle("is-selected", view.selected === item.key);
    card.setAttribute("aria-pressed", String(view.selected === item.key));
    setText(card.querySelector(".node-topology-card-role"), roleTitle(item.role));
    setText(card.querySelector(".node-topology-card-status"), item.status);
    setText(card.querySelector(".node-topology-card-name"), item.name);
    setText(card.querySelector(".node-topology-card-id"), item.id);
    setText(card.querySelector(".node-topology-card-note"), item.note);
    card.title = item.name + "\n" + item.id + "\n" + item.status;
  }
  const nextEdges = new Set(model.edges.map(e => e.key));
  for (const [key, parts] of view.paths) {
    if (!nextEdges.has(key)) {
      parts.path.remove(); parts.label.remove(); parts.event?.remove(); view.paths.delete(key);
    }
  }
  for (const edge of model.edges) {
    let parts = view.paths.get(edge.key);
    if (!parts) {
      const path = svgElement("path", {
        class: "node-topology-link node-topology-link--" + edge.kind,
        "marker-end": "url(#node-topology-arrow-" + edge.kind + ")"
      });
      const label = svgElement("text", {class: "node-topology-edge-label node-topology-edge-label--" + edge.kind,
        "text-anchor": "middle"});
      label.textContent = edge.label;
      const event = edge.kind === "request" || edge.kind === "response" ?
        svgElement("text", {
          class: "node-topology-flow-meta node-topology-flow-meta--" + edge.kind,
          "text-anchor": "middle"
        }) : null;
      if (event) view.svg.append(path, label, event);
      else view.svg.append(path, label);
      parts = {path, label, event};
      view.paths.set(edge.key, parts);
    }
    const dimmed = view.selected !== "local" && edge.peerKey !== view.selected;
    parts.path.classList.toggle("is-dimmed", dimmed);
    parts.label.classList.toggle("is-dimmed", dimmed);
    parts.event?.classList.toggle("is-dimmed", dimmed);
    const peer=model.nodes.find(item=>item.key===edge.peerKey);
    const unreachable=peer?.connectivity==="unreachable";
    parts.path.classList.toggle("is-unreachable", unreachable);
    parts.label.classList.toggle("is-unreachable", unreachable);
    parts.event?.classList.toggle("is-unreachable", unreachable);
  }
  if (view.discoveredY !== null) {
    if (!view.sectionLabel) {
      view.sectionLabel = element("div", "node-topology-section-label",
        "LAN 广播发现 · 未配对 / 无可信数据通道");
      view.scene.append(view.sectionLabel);
    }
  } else {
    view.sectionLabel?.remove();
    view.sectionLabel = null;
  }

  drawGeometry();
  renderFlowLabels();
  if (previousIds !== nextIds && !view.userMoved) fitTopology();
  if (!model.nodes.some(n => n.key === view.selected)) view.selected = "local";
  showDetail(model.nodes.find(n => n.key === view.selected));
  setText(document.getElementById("node-topology-count"),
    model.counts.trusted + " 已配对 · " + model.counts.online + " 在线 · " +
    model.counts.unreachable + " 不可达 · " + model.counts.unsupported + " 旧版待升级 · " + model.counts.pending +
    " 待确认 · " + model.counts.discovered + " 仅发现");
  const hint = !discovery.enabled ? "Transfer 未启用；可在 Config 中启用后重启服务。" :
    model.counts.unreachable ? model.counts.unreachable+" 个子节点 TLS 探测连续失败（可能服务关闭或网络中断），配对和授权保持不变。" :
    model.counts.unsupported ? model.counts.unsupported+" 个旧版子节点不支持健康探测，连接状态未验证；升级子节点后可自动识别在线。" :
    model.counts.trusted ? "已配对节点仅在 TLS 验证通过时标记在线；没有 LAN 广播不等于离线。" :
    "尚无可信数据通道；先完成配对，再单独授权 Project。";
  setText(document.getElementById("node-topology-summary"), hint);
}
