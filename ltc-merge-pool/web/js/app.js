// HappyChina Pool - Frontend Application
// Auto-refreshes every 15 seconds

const API_BASE = "/api";
let refreshTimer = null;

// ── Utility Functions ──────────────────────────────

function formatHashrate(h) {
    if (!h || h <= 0) return "0 H/s";
    const units = ["H/s", "KH/s", "MH/s", "GH/s", "TH/s", "PH/s", "EH/s"];
    let idx = 0;
    let v = h;
    while (v >= 1000 && idx < units.length - 1) { v /= 1000; idx++; }
    return v.toFixed(2) + " " + units[idx];
}

function formatNumber(n, decimals) {
    if (n === undefined || n === null) return "0";
    decimals = decimals || 0;
    return Number(n).toLocaleString(undefined, {
        minimumFractionDigits: decimals,
        maximumFractionDigits: decimals
    });
}

function formatCoin(amount, symbol) {
    if (symbol === "LTC" || symbol === "BELLS" || symbol === "LKY") {
        return formatNumber(amount, 8);
    }
    return formatNumber(amount, 4);
}

function timeAgo(dateStr) {
    const now = Date.now();
    const then = new Date(dateStr).getTime();
    const diff = Math.floor((now - then) / 1000);
    if (diff < 60) return diff + "s ago";
    if (diff < 3600) return Math.floor(diff / 60) + "m ago";
    if (diff < 86400) return Math.floor(diff / 3600) + "h ago";
    return Math.floor(diff / 86400) + "d ago";
}

function shortHash(hash) {
    if (!hash || hash.length < 16) return hash || "";
    return hash.substring(0, 8) + "..." + hash.substring(hash.length - 8);
}

function statusBadge(status) {
    var cls = "badge-pending";
    if (status === "confirmed") cls = "badge-confirmed";
    else if (status === "orphaned") cls = "badge-orphaned";
    return '<span class="badge ' + cls + '">' + status + '</span>';
}

function coinIconClass(symbol) {
    return "coin-" + symbol.toLowerCase();
}

function showToast(msg, type) {
    type = type || "success";
    var toast = document.getElementById("toast");
    if (!toast) {
        toast = document.createElement("div");
        toast.id = "toast";
        toast.className = "toast";
        document.body.appendChild(toast);
    }
    toast.textContent = msg;
    toast.className = "toast show " + type;
    setTimeout(function() { toast.className = "toast"; }, 4000);
}

async function apiFetch(path) {
    try {
        var resp = await fetch(API_BASE + path);
        if (!resp.ok) throw new Error("HTTP " + resp.status);
        return await resp.json();
    } catch (e) {
        console.error("API error:", path, e);
        return null;
    }
}

async function apiPost(path, body) {
    try {
        var resp = await fetch(API_BASE + path, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(body)
        });
        return await resp.json();
    } catch (e) {
        console.error("API POST error:", path, e);
        return { success: false, message: "Network error" };
    }
}

function formatOdds(ratio) {
    if (!ratio || ratio <= 0 || !isFinite(ratio)) return "--";
    if (ratio >= 1) return "1:1";
    var x = 1 / ratio;
    if (x >= 1000000) return "1:" + (x / 1000000).toFixed(1) + "M";
    if (x >= 1000) return "1:" + (x / 1000).toFixed(1) + "K";
    return "1:" + x.toFixed(1);
}

function formatDuration(secs) {
    if (!secs || secs <= 0 || !isFinite(secs)) return "N/A";
    var total = Math.floor(secs);
    if (total < 60) return total + "s";
    if (total < 3600) return Math.floor(total / 60) + "m " + (total % 60) + "s";
    if (total < 86400) return Math.floor(total / 3600) + "h " + Math.floor((total % 3600) / 60) + "m";
    var days = Math.floor(total / 86400);
    var hours = Math.floor((total % 86400) / 3600);
    if (days > 365) return (days / 365).toFixed(1) + "y";
    return days + "d " + hours + "h";
}

function formatDifficultyShort(d) {
    if (!d || d <= 0) return "0";
    if (d >= 1e12) return (d / 1e12).toFixed(2) + "T";
    if (d >= 1e9) return (d / 1e9).toFixed(2) + "G";
    if (d >= 1e6) return (d / 1e6).toFixed(2) + "M";
    if (d >= 1e3) return (d / 1e3).toFixed(2) + "K";
    return d.toFixed(2);
}

// ── Dashboard (index.html) ─────────────────────────

async function loadDashboard() {
    var data = await apiFetch("/pool");
    if (!data) return;

    setText("stat-hashrate", data.hashrate_formatted || "0 H/s");
    setText("stat-miners", formatNumber(data.miners));
    setText("stat-workers", formatNumber(data.workers));
    setText("stat-netdiff", formatNumber(data.network_difficulty, 2));
    setText("stat-ttf", data.est_time_to_find || "N/A");
    setText("stat-blocks", formatNumber(data.blocks_found));
    setText("stat-shares", formatNumber(data.total_shares));
    setText("stat-sps", formatNumber(data.shares_per_sec, 2));
    setText("stat-fee", data.fee_percent + "%");

    // Odds / TTF bar calculations
    var hashrate = data.hashrate || 0;
    var netDiff = data.network_difficulty || 0;
    var bestShare = data.best_share || 0;

    // Pool hashrate display
    setText("odds-hashrate", formatHashrate(hashrate));

    // TTF in seconds: netDiff * 2^32 / hashrate
    // Block time for LTC = 150s (2.5 min)
    var blockTime = 150;
    var ttfSecs = 0;
    var oddsPerBlock = 0;

    if (hashrate > 0 && netDiff > 0) {
        ttfSecs = netDiff * 4294967296 / hashrate;
        // Odds per block = blockTime / ttfSecs
        oddsPerBlock = blockTime / ttfSecs;
    }

    setText("odds-per-block", formatOdds(oddsPerBlock));
    setText("odds-per-day", formatOdds(oddsPerBlock * 86400 / blockTime));
    setText("odds-per-week", formatOdds(oddsPerBlock * 604800 / blockTime));
    setText("odds-per-year", formatOdds(oddsPerBlock * 31536000 / blockTime));
    setText("odds-ttf", formatDuration(ttfSecs));
    setText("odds-best-share", formatDifficultyShort(bestShare));
    setText("odds-net-diff", formatDifficultyShort(netDiff));

    // Coins grid
    var coinsEl = document.getElementById("coins-grid");
    if (coinsEl && data.coins) {
        var html = "";
        for (var i = 0; i < data.coins.length; i++) {
            var c = data.coins[i];
            var badgeCls = c.is_parent ? "parent" : "aux";
            var badgeText = c.is_parent ? "Parent" : "Aux";
            html += '<div class="coin-card">';
            html += '<div class="coin-header">';
            html += '<div class="coin-icon ' + coinIconClass(c.symbol) + '">' + c.symbol.substring(0, 3) + '</div>';
            html += '<div><div class="coin-name">' + c.name + ' <span class="coin-badge ' + badgeCls + '">' + badgeText + '</span></div>';
            html += '<div class="coin-symbol">' + c.symbol + '</div></div></div>';
            html += '<div class="coin-detail"><span class="detail-label">Difficulty</span><span class="detail-value">' + formatNumber(c.difficulty, 4) + '</span></div>';
            html += '<div class="coin-detail"><span class="detail-label">Height</span><span class="detail-value">' + formatNumber(c.height) + '</span></div>';
            html += '<div class="coin-detail"><span class="detail-label">Reward</span><span class="detail-value">' + formatCoin(c.block_reward, c.symbol) + ' ' + c.symbol + '</span></div>';
            html += '</div>';
        }
        coinsEl.innerHTML = html;
    }

    // Recent blocks
    var blocksData = await apiFetch("/blocks");
    if (blocksData && blocksData.blocks) {
        var tbody = document.getElementById("recent-blocks-body");
        if (tbody) {
            var rows = blocksData.blocks.slice(0, 20);
            if (rows.length === 0) {
                tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;color:var(--text-muted);padding:2rem">No blocks found yet</td></tr>';
            } else {
                var rhtml = "";
                for (var j = 0; j < rows.length; j++) {
                    var b = rows[j];
                    rhtml += '<tr>';
                    rhtml += '<td><span class="coin-icon ' + coinIconClass(b.coin) + '" style="width:24px;height:24px;display:inline-flex;font-size:0.6rem;vertical-align:middle">' + b.coin.substring(0,3) + '</span> ' + b.coin + '</td>';
                    rhtml += '<td class="mono">' + formatNumber(b.height) + '</td>';
                    rhtml += '<td class="hash-cell"><a href="blocks.html?coin=' + b.coin + '">' + shortHash(b.block_hash || b.hash) + '</a></td>';
                    rhtml += '<td class="mono">' + formatCoin(b.reward, b.coin) + ' ' + b.coin + '</td>';
                    rhtml += '<td>' + timeAgo(b.created_at) + '</td>';
                    rhtml += '<td>' + statusBadge(b.status) + '</td>';
                    rhtml += '</tr>';
                }
                tbody.innerHTML = rhtml;
            }
        }
    }
}

// ── Miner Page (miner.html) ────────────────────────

async function loadMinerPage() {
    var params = new URLSearchParams(window.location.search);
    var addr = params.get("addr");
    if (!addr) {
        document.getElementById("miner-content").innerHTML = '<div class="loading">Enter a miner address to view stats.</div>';
        return;
    }

    document.getElementById("miner-addr-display").textContent = addr;

    var data = await apiFetch("/miner/" + encodeURIComponent(addr));
    if (!data) {
        document.getElementById("miner-content").innerHTML = '<div class="loading">Failed to load miner data.</div>';
        return;
    }

    setText("miner-hashrate", data.hashrate_formatted || "0 H/s");
    setText("miner-workers-count", data.workers_online + " / " + data.worker_count);

    // Workers
    var workersData = await apiFetch("/miner/" + encodeURIComponent(addr) + "/workers");
    var workersTbody = document.getElementById("workers-body");
    if (workersTbody && workersData && workersData.workers) {
        if (workersData.workers.length === 0) {
            workersTbody.innerHTML = '<tr><td colspan="6" style="text-align:center;color:var(--text-muted);padding:2rem">No workers</td></tr>';
        } else {
            var whtml = "";
            for (var k = 0; k < workersData.workers.length; k++) {
                var w = workersData.workers[k];
                var statusCls = w.is_online ? "badge-online" : "badge-offline";
                var statusTxt = w.is_online ? "Online" : "Offline";
                whtml += '<tr>';
                whtml += '<td class="mono">' + w.name + '</td>';
                whtml += '<td class="mono">' + w.hashrate_formatted + '</td>';
                whtml += '<td class="mono">' + formatNumber(w.difficulty) + '</td>';
                whtml += '<td>' + timeAgo(w.last_seen) + '</td>';
                whtml += '<td><span class="badge ' + statusCls + '">' + statusTxt + '</span></td>';
                whtml += '<td class="mono" style="font-size:0.75rem;color:var(--text-muted)">' + (w.user_agent || "-") + '</td>';
                whtml += '</tr>';
            }
            workersTbody.innerHTML = whtml;
        }
    }

    // Assets link
    var assetsLink = document.getElementById("assets-link");
    if (assetsLink) assetsLink.href = "assets.html?addr=" + encodeURIComponent(addr);
}

// ── Assets Page (assets.html) ──────────────────────

async function loadAssetsPage() {
    var params = new URLSearchParams(window.location.search);
    var addr = params.get("addr");
    if (!addr) {
        document.getElementById("assets-content").innerHTML = '<div class="loading">No miner address specified.</div>';
        return;
    }

    document.getElementById("assets-addr").textContent = addr;

    var data = await apiFetch("/miner/" + encodeURIComponent(addr));
    if (!data) return;

    var allCoins = ["LTC","DOGE","PEPE","BELLS","LKY","JKC","DINGO","SHIC","TRMP"];
    var balMap = {};
    if (data.balances) {
        for (var i = 0; i < data.balances.length; i++) {
            balMap[data.balances[i].coin] = data.balances[i].amount;
        }
    }

    var tbody = document.getElementById("assets-body");
    if (tbody) {
        var ahtml = "";
        for (var j = 0; j < allCoins.length; j++) {
            var sym = allCoins[j];
            var bal = balMap[sym] || 0;
            ahtml += '<tr>';
            ahtml += '<td><div class="asset-coin-cell"><div class="coin-icon ' + coinIconClass(sym) + '">' + sym.substring(0,3) + '</div><div><div style="font-weight:600">' + sym + '</div></div></div></td>';
            ahtml += '<td class="mono">' + formatCoin(bal, sym) + '</td>';
            ahtml += "<td><button class=\"btn btn-outline btn-sm\" onclick=\"openWithdrawModal('" + addr + "','" + sym + "'," + bal + ")\">Withdraw</button></td>";
            ahtml += '</tr>';
        }
        tbody.innerHTML = ahtml;
    }
}

function openWithdrawModal(addr, coin, balance) {
    document.getElementById("withdraw-coin").textContent = coin;
    document.getElementById("withdraw-balance").textContent = formatCoin(balance, coin) + " " + coin;
    document.getElementById("withdraw-amount").value = "";
    document.getElementById("withdraw-addr").value = addr;
    document.getElementById("withdraw-coin-hidden").value = coin;
    document.getElementById("withdraw-max").value = balance;
    document.getElementById("modal-overlay").classList.add("active");
}

function closeModal() {
    document.getElementById("modal-overlay").classList.remove("active");
}

function setMaxWithdraw() {
    var max = parseFloat(document.getElementById("withdraw-max").value) || 0;
    document.getElementById("withdraw-amount").value = max;
}

async function submitWithdrawal() {
    var miner = document.getElementById("withdraw-addr").value;
    var coin = document.getElementById("withdraw-coin-hidden").value;
    var amount = parseFloat(document.getElementById("withdraw-amount").value);

    if (!amount || amount <= 0) {
        showToast("Enter a valid amount", "error");
        return;
    }

    var result = await apiPost("/withdraw", { miner: miner, coin: coin, amount: amount });
    closeModal();

    if (result.success) {
        showToast(result.message, "success");
        setTimeout(function() { loadAssetsPage(); }, 1500);
    } else {
        showToast(result.message || "Withdrawal failed", "error");
    }
}

// ── Blocks Page (blocks.html) ──────────────────────

var currentCoinFilter = "ALL";

async function loadBlocksPage() {
    var params = new URLSearchParams(window.location.search);
    var coin = params.get("coin");
    if (coin) currentCoinFilter = coin.toUpperCase();

    // Set initial active tab
    document.querySelectorAll(".filter-tab").forEach(function(el) {
        el.classList.toggle("active", el.dataset.coin === currentCoinFilter);
    });

    await refreshBlocks();
}

async function filterBlocks(coin) {
    currentCoinFilter = coin;
    document.querySelectorAll(".filter-tab").forEach(function(el) {
        el.classList.toggle("active", el.dataset.coin === coin);
    });
    await refreshBlocks();
}

async function refreshBlocks() {
    var path = currentCoinFilter === "ALL" ? "/blocks" : "/blocks/" + currentCoinFilter;
    var data = await apiFetch(path);
    if (!data) return;

    setText("blocks-total", "Total: " + formatNumber(data.total));

    var tbody = document.getElementById("blocks-body");
    if (tbody && data.blocks) {
        if (data.blocks.length === 0) {
            tbody.innerHTML = '<tr><td colspan="8" style="text-align:center;color:var(--text-muted);padding:2rem">No blocks found</td></tr>';
        } else {
            var bhtml = "";
            for (var i = 0; i < data.blocks.length; i++) {
                var b = data.blocks[i];
                bhtml += '<tr>';
                bhtml += '<td><span class="coin-icon ' + coinIconClass(b.coin) + '" style="width:24px;height:24px;display:inline-flex;font-size:0.6rem;vertical-align:middle">' + b.coin.substring(0,3) + '</span> ' + b.coin + '</td>';
                bhtml += '<td class="mono">' + formatNumber(b.height) + '</td>';
                bhtml += '<td class="hash-cell">' + shortHash(b.block_hash || b.hash) + '</td>';
                bhtml += '<td class="hash-cell" style="max-width:140px">' + shortHash(b.miner) + '</td>';
                bhtml += '<td class="mono">' + formatCoin(b.reward, b.coin) + ' ' + b.coin + '</td>';
                bhtml += '<td class="mono">' + b.confirmations + '</td>';
                bhtml += '<td>' + statusBadge(b.status) + '</td>';
                bhtml += '<td>' + timeAgo(b.created_at) + '</td>';
                bhtml += '</tr>';
            }
            tbody.innerHTML = bhtml;
        }
    }
}

// ── Helpers ─────────────────────────────────────────

function setText(id, text) {
    var el = document.getElementById(id);
    if (el) el.textContent = text;
}

function goToMiner() {
    var addr = document.getElementById("miner-lookup-input").value.trim();
    if (addr) {
        window.location.href = "miner.html?addr=" + encodeURIComponent(addr);
    }
}

function handleMinerKeypress(e) {
    if (e.key === "Enter") goToMiner();
}

// ── Auto-refresh ───────────────────────────────────

function startAutoRefresh(loadFn, intervalMs) {
    intervalMs = intervalMs || 15000;
    loadFn();
    if (refreshTimer) clearInterval(refreshTimer);
    refreshTimer = setInterval(loadFn, intervalMs);
}
