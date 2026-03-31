// ── Web Crypto helpers ────────────────────────────────────────────────────────
// Format: base64( salt[16] || iv[12] || ciphertext )
// Key derivation: PBKDF2-SHA256, 100 000 iterations → AES-256-GCM key.

async function cryptoEncrypt(plaintext, password) {
    var enc  = new TextEncoder();
    var salt = crypto.getRandomValues(new Uint8Array(16));
    var iv   = crypto.getRandomValues(new Uint8Array(12));

    var keyMaterial = await crypto.subtle.importKey(
        "raw", enc.encode(password), "PBKDF2", false, ["deriveKey"]
    );
    var key = await crypto.subtle.deriveKey(
        { name: "PBKDF2", salt: salt, iterations: 1000000, hash: "SHA-256" },
        keyMaterial,
        { name: "AES-GCM", length: 256 },
        false, ["encrypt"]
    );

    var ciphertext = await crypto.subtle.encrypt(
        { name: "AES-GCM", iv: iv }, key, enc.encode(plaintext)
    );

    var result = new Uint8Array(16 + 12 + ciphertext.byteLength);
    result.set(salt, 0);
    result.set(iv, 16);
    result.set(new Uint8Array(ciphertext), 28);

    var binary = "";
    for (var i = 0; i < result.length; i++) { binary += String.fromCharCode(result[i]); }
    return btoa(binary);
}

async function cryptoDecrypt(ciphertextB64, password) {
    var enc  = new TextEncoder();
    var data = Uint8Array.from(atob(ciphertextB64), function(c) { return c.charCodeAt(0); });

    var salt       = data.slice(0, 16);
    var iv         = data.slice(16, 28);
    var ciphertext = data.slice(28);

    var keyMaterial = await crypto.subtle.importKey(
        "raw", enc.encode(password), "PBKDF2", false, ["deriveKey"]
    );
    var key = await crypto.subtle.deriveKey(
        { name: "PBKDF2", salt: salt, iterations: 1000000, hash: "SHA-256" },
        keyMaterial,
        { name: "AES-GCM", length: 256 },
        false, ["decrypt"]
    );

    var plaintext = await crypto.subtle.decrypt(
        { name: "AES-GCM", iv: iv }, key, ciphertext
    );
    return new TextDecoder().decode(plaintext);
}
// ─────────────────────────────────────────────────────────────────────────────

$(document).ready(function() {
    // ── Dark mode toggle ──────────────────────────────────────────────────────
    function isDark() {
        var theme = document.documentElement.getAttribute('data-theme');
        if (theme === 'dark') return true;
        if (theme === 'light') return false;
        return window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches;
    }

    function applyTheme(dark) {
        document.documentElement.setAttribute('data-theme', dark ? 'dark' : 'light');
        $('#theme-toggle i').toggleClass('fa-moon', !dark).toggleClass('fa-sun', dark);
    }

    applyTheme(isDark());

    $('#theme-toggle').on('click', function () {
        var dark = !isDark();
        localStorage.setItem('theme', dark ? 'dark' : 'light');
        applyTheme(dark);
        if (typeof Prism !== 'undefined') {
            Prism.highlightAll();
        }
    });

    // Re-sync icon if OS preference changes while the page is open.
    if (window.matchMedia) {
        window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', function () {
            if (!localStorage.getItem('theme')) {
                applyTheme(isDark());
            }
        });
    }
    // ─────────────────────────────────────────────────────────────────────────

    function replaceUrlParam(url, param, value) {
        if (value == null) {
            value = '';
        }

        var pattern = new RegExp('\\b('+param+'=).*?(&|#|$)');
        if (url.search(pattern)>=0) {
            return url.replace(pattern,'$1' + value + '$2');
        }

        url = url.replace(/[?#]$/,'');
        return url + (url.indexOf('?')>0 ? '&' : '?') + param + '=' + value;
    }

    function resetLanguageSelector() {
        var url = new URL(document.location);
        var params = url.searchParams;
        var lang = params.get("lang");

        if (lang != null) {
            $("#language-selector").val(lang);
        } else {
            if($("#pastebin-code-block").length) {
                $("#language-selector").val(
                    $("#pastebin-code-block").prop("class").trim().split('-')[1]
                );
            }
        }
    }

    function getDefaultExpiryTime() {
        var expiry = $("#expiry-dropdown-btn").text().split("Expires: ")[1];
        return $("#expiry-dropdown a:contains('"+ expiry +"')").attr('href');
    }

    function checkPasswordModal() {
        if ($("#password-modal").length) {
            $('#password-modal').modal('toggle');
        }
    }

    resetLanguageSelector();
    checkPasswordModal();
    init_plugins();

    var state = {
        expiry: getDefaultExpiryTime(),
        burn: 0,
    };

    $("#language-selector").change(function() {
        if ($("#pastebin-code-block").length) {
            $('#pastebin-code-block').attr('class', 'language-' + $("#language-selector").val());
            init_plugins();
        }
    });

    $("#remove-btn").on("click", function(event) {
        event.preventDefault();
        $('#deletion-modal').modal('show');
    });

    $("#deletion-confirm-btn").on("click", function(event) {
        event.preventDefault();

        $.ajax({
            url: window.location.pathname,
            type: 'DELETE',
            success: function(result) {
                uri = uri_prefix + "/new";
                uri = replaceUrlParam(uri, 'level', "info");
                uri = replaceUrlParam(uri, 'glyph', "fas fa-info-circle");
                uri = replaceUrlParam(uri, 'msg', "The paste has been successfully removed.");
                window.location.href = encodeURI(uri);
            }
        });
    });

    $("#copy-btn").on("click", function(event) {
        event.preventDefault();

        var $this = $(this);
        var text = $("#pastebin-code-block").text();

        navigator.clipboard.writeText(text).then(function() {
            $this.text("Copied!");
            $this.attr("disabled", "disabled");

            setTimeout(function() {
                $this.text("Copy");
                $this.removeAttr("disabled");
            }, 800);
        });
    });

    $("#send-btn").on("click", function(event) {
        event.preventDefault();

        var uri = uri_prefix == "" ? "/" : uri_prefix;
        uri = replaceUrlParam(uri, 'lang', $("#language-selector").val());
        uri = replaceUrlParam(uri, 'ttl', state.expiry);
        uri = replaceUrlParam(uri, 'burn', state.burn);

        var data = $("#content-textarea").val();
        var pass = $("#pastebin-password").val();

        function doPost(payload) {
            $.ajax({
                url: uri,
                type: 'POST',
                data: payload,
                success: function(result) {
                    var dest = uri_prefix + "/new";
                    dest = replaceUrlParam(dest, 'level', "success");
                    dest = replaceUrlParam(dest, 'glyph', "fas fa-check");
                    dest = replaceUrlParam(dest, 'msg', "The paste has been successfully created:");
                    dest = replaceUrlParam(dest, 'url', result);
                    window.location.href = encodeURI(dest);
                }
            });
        }

        if (pass.length > 0) {
            cryptoEncrypt(data, pass).then(function(encrypted) {
                uri = replaceUrlParam(uri, 'encrypted', true);
                doPost(encrypted);
            });
        } else {
            doPost(data);
        }
    });

    $('#expiry-dropdown a').click(function(event){
        event.preventDefault();

        state.expiry = $(this).attr("href");
        $('#expiry-dropdown-btn').text("Expires: " + this.innerHTML);
    });

    $('#burn-dropdown a').click(function(event){
        event.preventDefault();

        state.burn = $(this).attr("href");
        $('#burn-dropdown-btn').text("Burn: " + this.innerHTML);
    });

    $('#password-modal').on('shown.bs.modal', function () {
        $('#modal-password').trigger('focus');
    })

    $('#password-modal form').submit(function(event) {
        event.preventDefault();
        $('#decrypt-btn').click();
    });

    $('#decrypt-btn').click(function(event) {
        var pass = $("#modal-password").val();
        var data = "";

        if ($("#pastebin-code-block").length) {
            data = $("#pastebin-code-block").text();
        } else {
            data = $("#content-textarea").text();
        }

        cryptoDecrypt(data, pass).then(function(decrypted) {
            if ($("#pastebin-code-block").length) {
                $("#pastebin-code-block").text(decrypted);
                init_plugins();
            } else {
                $("#content-textarea").text(decrypted);
            }

            $("#modal-close-btn").click();
            $("#modal-alert").alert('close');
        }).catch(function() {
            $("#modal-alert").removeClass("collapse");
        });
    });
});
