/**
 * QuickMD - Application Client Logic
 * Handles Markdown rendering, KaTeX math, Mermaid diagrams, Highlight.js,
 * TOC outline with ScrollSpy, in-page search, theme management, and file watching.
 */

(function () {
  'use strict';

  // --- State Management ---
  const state = {
    currentFile: null,
    rawContent: '',
    stats: {},
    theme: 'dark',
    contentWidth: 'normal',
    fontSize: 16,
    sidebarOpen: true,
    recentFiles: [],
    searchMatches: [],
    currentMatchIndex: -1,
  };

  const THEMES = ['dark', 'light', 'tokyo-night', 'sepia', 'oled'];
  const WIDTHS = ['compact', 'normal', 'wide', 'full'];
  const headingSlugCounts = new Map();

  // --- DOM Elements ---
  const el = {
    appContainer: document.getElementById('app-container'),
    readingProgressBar: document.getElementById('reading-progress-bar'),
    breadcrumbDir: document.getElementById('breadcrumb-dir'),
    breadcrumbSeparator: document.getElementById('breadcrumb-separator'),
    breadcrumbFile: document.getElementById('breadcrumb-file'),
    btnOpenFile: document.getElementById('btn-open-file'),
    btnToggleSidebar: document.getElementById('btn-toggle-sidebar'),
    btnSearch: document.getElementById('btn-search'),
    btnInfo: document.getElementById('btn-info'),
    btnStats: document.getElementById('btn-stats'),
    btnWidth: document.getElementById('btn-width'),
    btnTheme: document.getElementById('btn-theme'),
    btnZoomIn: document.getElementById('btn-zoom-in'),
    btnZoomOut: document.getElementById('btn-zoom-out'),
    btnZoomReset: document.getElementById('btn-zoom-reset'),
    zoomLabel: document.getElementById('zoom-label'),
    btnCopyRaw: document.getElementById('btn-copy-raw'),
    btnPrint: document.getElementById('btn-print'),
    sidebar: document.getElementById('sidebar'),
    outlineCount: document.getElementById('outline-count'),
    tocContainer: document.getElementById('toc-container'),
    recentFilesList: document.getElementById('recent-files-list'),
    contentScroller: document.getElementById('content-scroller'),
    welcomeScreen: document.getElementById('welcome-screen'),
    dropzone: document.getElementById('dropzone'),
    btnWelcomeOpen: document.getElementById('btn-welcome-open'),
    markdownArticle: document.getElementById('markdown-article'),
    frontmatterContainer: document.getElementById('frontmatter-container'),
    articleBody: document.getElementById('article-body'),
    searchModal: document.getElementById('search-modal'),
    searchInput: document.getElementById('search-input'),
    searchCount: document.getElementById('search-count'),
    btnSearchPrev: document.getElementById('btn-search-prev'),
    btnSearchNext: document.getElementById('btn-search-next'),
    btnSearchClose: document.getElementById('btn-search-close'),
    statsModalOverlay: document.getElementById('stats-modal-overlay'),
    btnStatsClose: document.getElementById('btn-stats-close'),
    aboutModalOverlay: document.getElementById('about-modal-overlay'),
    btnAboutClose: document.getElementById('btn-about-close'),
    statWords: document.getElementById('stat-words'),
    statReadtime: document.getElementById('stat-readtime'),
    statChars: document.getElementById('stat-chars'),
    statLines: document.getElementById('stat-lines'),
    statHeadings: document.getElementById('stat-headings'),
    statCodeblocks: document.getElementById('stat-codeblocks'),
    lightboxOverlay: document.getElementById('lightbox-overlay'),
    lightboxImg: document.getElementById('lightbox-img'),
    statusWords: document.getElementById('status-words'),
    statusReadtime: document.getElementById('status-readtime'),
    statusSize: document.getElementById('status-size'),
    statusEncoding: document.getElementById('status-encoding'),
    statusMtime: document.getElementById('status-mtime'),
    highlightThemeLink: document.getElementById('highlight-theme-link'),
    btnWindowsAssoc: document.getElementById('btn-windows-assoc'),
    btnSetupWindows: document.getElementById('btn-setup-windows'),
    windowsAssocBanner: document.getElementById('windows-assoc-banner'),
    dropOverlay: document.getElementById('drop-overlay'),
  };

  // --- Logger Bridge (Fire-and-forget, non-blocking) ---
  function logJS(msg) {
    console.log(msg);
    if (window.pywebview && window.pywebview.api && window.pywebview.api.log_from_js) {
      try {
        window.pywebview.api.log_from_js(msg);
      } catch (e) {}
    }
  }

  // --- Initialize Application ---
  let isInitialized = false;
  async function safeInit() {
    if (isInitialized) return;
    if (window.pywebview && window.pywebview.api) {
      isInitialized = true;
      logJS("safeInit triggered bridge initialization");
      await initializeFromBackend();
    }
  }

  window.addEventListener('DOMContentLoaded', () => {
    initMermaid();
    setupMarkedExtensions();
    setupEventListeners();
    updateIcons();
    safeInit();
  });

  window.addEventListener('pywebviewready', () => {
    safeInit();
  });

  // Polling fallback to guarantee bridge hook if event fired before listener
  const initInterval = setInterval(() => {
    if (isInitialized) {
      clearInterval(initInterval);
    } else {
      safeInit();
    }
  }, 100);
  setTimeout(() => clearInterval(initInterval), 4000);

  function updateIcons() {
    if (window.lucide) {
      window.lucide.createIcons();
    }
  }

  let mermaidLoadingPromise = null;
  function ensureMermaidLoaded() {
    if (window.mermaid) {
      initMermaid();
      return Promise.resolve(window.mermaid);
    }
    if (mermaidLoadingPromise) {
      return mermaidLoadingPromise;
    }
    mermaidLoadingPromise = new Promise((resolve) => {
      const script = document.createElement('script');
      script.src = '../js/mermaid.min.js';
      script.onload = () => {
        initMermaid();
        resolve(window.mermaid);
      };
      script.onerror = () => {
        mermaidLoadingPromise = null;
        resolve(null);
      };
      document.head.appendChild(script);
    });
    return mermaidLoadingPromise;
  }

  function initMermaid() {
    if (window.mermaid) {
      window.mermaid.initialize({
        startOnLoad: false,
        theme: state.theme === 'light' || state.theme === 'sepia' ? 'default' : 'dark',
        securityLevel: 'strict',
        fontFamily: 'var(--font-sans)',
      });
    }
  }

  // --- Backend Bridge Integration ---
  async function initializeFromBackend() {
    logJS("initializeFromBackend() called");
    try {
      const initData = await window.pywebview.api.get_initial_data();
      logJS("initData received: has_initial_file=" + (initData ? initData.has_initial_file : "null") + ", file=" + (initData && initData.file_data ? initData.file_data.filename : "none"));
      if (initData) {
        try {
          if (initData.config) {
            applySettings(initData.config);
          }
        } catch (cfgErr) {
          console.warn('Config apply error:', cfgErr);
        }

        // Windows Integration Banner/Button visibility logic:
        try {
          if (initData.is_windows_registered === false) {
            if (el.windowsAssocBanner) el.windowsAssocBanner.style.display = 'flex';
            if (el.btnWindowsAssoc) el.btnWindowsAssoc.style.display = 'inline-flex';
          } else {
            if (el.windowsAssocBanner) el.windowsAssocBanner.style.display = 'none';
            if (el.btnWindowsAssoc) el.btnWindowsAssoc.style.display = 'none';
          }
        } catch (winErr) {
          console.warn('Windows banner error:', winErr);
        }

        if (initData.has_initial_file && initData.file_data) {
          await renderFile(initData.file_data);
        } else {
          renderRecentFiles(initData.config ? (initData.config.recent_files || []) : []);
        }
      }
    } catch (err) {
      logJS("initializeFromBackend error: " + err);
      console.error('Initialization error:', err);
    } finally {
      requestAnimationFrame(() => {
        if (window.pywebview && window.pywebview.api && window.pywebview.api.show_window) {
          window.pywebview.api.show_window();
        }
      });
    }
  }

  // --- Marked Parser Configuration ---
  function setupMarkedExtensions() {
    const renderer = new marked.Renderer();

    // Custom Code Block Renderer (Mermaid or Syntax Highlighting + Copy button)
    renderer.code = function (token) {
      const text = (typeof token === 'object' ? token.text : token) || '';
      const lang = ((typeof token === 'object' ? token.lang : '') || 'text').trim().toLowerCase();

      if (lang === 'mermaid') {
        return `<div class="mermaid-container" data-mermaid-raw="${encodeURIComponent(text)}"><pre class="mermaid">${escapeHtml(text)}</pre></div>`;
      }

      const validLang = hljs.getLanguage(lang) ? lang : 'plaintext';
      const highlighted = hljs.highlight(text, { language: validLang, ignoreIllegals: true }).value;

      return `
        <div class="code-block-container">
          <div class="code-header">
            <span>${escapeHtml(lang)}</span>
            <button class="code-copy-btn" title="Copy Code">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>
              <span>Copy</span>
            </button>
          </div>
          <pre><code class="hljs language-${validLang}">${highlighted}</code></pre>
        </div>
      `;
    };

    // Custom Table Renderer (Scrollable wrapper + Copy button)
    renderer.table = function (token) {
      const tableHtml = marked.Renderer.prototype.table.call(this, token);
      return `
        <div class="table-wrapper">
          <button class="copy-table-btn" title="Copy Table as Markdown">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"></path><rect x="8" y="2" width="8" height="4" rx="1" ry="1"></rect></svg>
            <span>Copy Table</span>
          </button>
          ${tableHtml}
        </div>
      `;
    };

    // Custom Blockquote Renderer for GitHub / Obsidian Admonitions (Callouts)
    renderer.blockquote = function (token) {
      const rawText = (typeof token === 'object' ? token.text : token) || '';
      const trimmed = rawText.trim();
      const calloutMatch = trimmed.match(/^\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION|INFO|SUCCESS|QUESTION|FAILURE|BUG|EXAMPLE|QUOTE)\](?:\s+(.*?))?(?:\n([\s\S]*))?$/i);

      if (calloutMatch) {
        const type = calloutMatch[1].toUpperCase();
        let title = calloutMatch[2] ? calloutMatch[2].trim() : type;
        const bodyText = calloutMatch[3] ? calloutMatch[3].trim() : '';

        // Map callout types to styling classes
        let typeClass = 'callout-note';
        let iconName = 'info';

        if (type === 'TIP' || type === 'SUCCESS') {
          typeClass = 'callout-tip';
          iconName = 'check-circle-2';
        } else if (type === 'IMPORTANT' || type === 'INFO') {
          typeClass = 'callout-important';
          iconName = 'sparkles';
        } else if (type === 'WARNING' || type === 'QUESTION') {
          typeClass = 'callout-warning';
          iconName = 'alert-triangle';
        } else if (type === 'CAUTION' || type === 'FAILURE' || type === 'BUG') {
          typeClass = 'callout-caution';
          iconName = 'alert-octagon';
        }

        const parsedBody = bodyText ? marked.parse(preprocessMarkdown(bodyText)) : '';

        return `
          <div class="callout ${typeClass}">
            <div class="callout-title">
              <i data-lucide="${iconName}"></i>
              <span>${escapeHtml(title)}</span>
            </div>
            <div class="callout-body">
              ${parsedBody}
            </div>
          </div>
        `;
      }

      return marked.Renderer.prototype.blockquote.call(this, token);
    };

    // Heading with IDs and Hover Anchors
    renderer.heading = function (token) {
      const text = (typeof token === 'object' ? token.text : token) || '';
      const depth = (typeof token === 'object' ? token.depth : 1) || 1;
      const cleanText = text.replace(/<[^>]+>/g, '');
      const baseSlug = cleanText
        .toLowerCase()
        .replace(/[^\w\u0E00-\u0E7F]+/g, '-')
        .replace(/^-+|-+$/g, '') || `heading-${Math.random().toString(36).substring(2, 7)}`;

      const count = headingSlugCounts.get(baseSlug) || 0;
      headingSlugCounts.set(baseSlug, count + 1);
      const slug = count > 0 ? `${baseSlug}-${count}` : baseSlug;

      const inlineHtml = (typeof token === 'object' && token.tokens && this.parser)
        ? this.parser.parseInline(token.tokens)
        : text;

      return `
        <h${depth} id="${slug}">
          <a class="heading-anchor" href="#${slug}" aria-hidden="true">#</a>
          ${inlineHtml}
        </h${depth}>
      `;
    };

    marked.setOptions({
      renderer: renderer,
      gfm: true,
      breaks: false,
      pedantic: false,
    });
  }

  // Pre-process custom markdown extensions (Highlights ==text==, ++underline++, Wikilinks [[...]])
  // Non-greedy stashing preserves fenced and inline code blocks without corrupting operators.
  function preprocessMarkdown(md) {
    if (!md) return '';

    const codeStash = [];
    const stashToken = (match) => {
      const idx = codeStash.length;
      codeStash.push(match);
      return `\u0000CODE_STASH_${idx}\u0000`;
    };

    // 1. Stash fenced code blocks (```...``` and ~~~...~~~) and inline code (`...`) non-greedily
    let processed = md.replace(/(?:^|\n)[ \t]*(?:`{3,}|~{3,})[\s\S]*?(?:^|\n)[ \t]*(?:`{3,}|~{3,})[ \t]*(?=\n|$)|(`+)([\s\S]*?)\1/g, stashToken);

    // 2. ==highlight== (single-line, non-whitespace boundaries)
    processed = processed.replace(/==([^\s=](?:[^=\n]*?[^\s=])?)==/g, '<mark>$1</mark>');

    // 3. ++underline++ (single-line, non-whitespace boundaries)
    processed = processed.replace(/\+\+([^\s+](?:[^+\n]*?[^\s+])?)\+\+/g, '<u>$1</u>');

    // 4. [[Wikilinks]] -> Link with data attributes (target & anchor)
    processed = processed.replace(/\[\[([^\]|\n]+)(?:\|([^\]\n]+))?\]\]/g, (match, fullTarget, alias) => {
      const label = alias || fullTarget;
      let target = (fullTarget || '').trim();
      let anchor = '';
      if (target.includes('#')) {
        const parts = target.split('#');
        target = parts[0].trim();
        anchor = parts.slice(1).join('#').trim();
      }
      return `<a class="wikilink" href="#" data-wikilink="${escapeHtml(target)}" data-anchor="${escapeHtml(anchor)}">${escapeHtml(label)}</a>`;
    });

    // 5. Restore stashed code blocks and inline code spans
    processed = processed.replace(/\u0000CODE_STASH_(\d+)\u0000/g, (_, idx) => codeStash[Number(idx)]);

    return processed;
  }

  // --- Comprehensive XSS & DOM Sanitizer for Local Markdown Rendering ---
  function sanitizeMarkdownHtml(dirtyHtml) {
    if (!dirtyHtml) return '';
    try {
      const parser = new DOMParser();
      const doc = parser.parseFromString(dirtyHtml, 'text/html');

      // 1. Strip dangerous tags (including executable SVG/MathML tags)
      const dangerousTags = [
        'script', 'iframe', 'object', 'embed', 'applet', 'meta', 'base', 'form',
        'style', 'link', 'svg', 'math', 'animate', 'set', 'handler'
      ];
      dangerousTags.forEach((tag) => {
        const elements = doc.querySelectorAll(tag);
        elements.forEach((node) => node.remove());
      });

      // 2. Deep clean dangerous attributes and schemes on all remaining nodes
      const allNodes = doc.querySelectorAll('*');
      allNodes.forEach((node) => {
        const attrs = Array.from(node.attributes);
        attrs.forEach((attr) => {
          const name = attr.name.toLowerCase();
          const rawVal = attr.value.trim();
          // Normalize value by stripping control chars and spaces for scheme inspection
          const normVal = rawVal.replace(/[\u0000-\u001F\s]+/g, '').toLowerCase();

          // Block all on* inline handlers
          if (name.startsWith('on')) {
            node.removeAttribute(attr.name);
            return;
          }

          // Block javascript:, vbscript:, and data: schemes on any URI attribute
          if (normVal.startsWith('javascript:') || normVal.startsWith('vbscript:') || normVal.startsWith('data:text/html')) {
            node.removeAttribute(attr.name);
            return;
          }

          // Strip xlink:href, formaction, and srcdoc attributes
          if (name.includes('href') && (normVal.startsWith('javascript:') || normVal.startsWith('vbscript:'))) {
            node.removeAttribute(attr.name);
            return;
          }
          if (name === 'formaction' || name === 'srcdoc') {
            node.removeAttribute(attr.name);
          }
        });
      });

      return doc.body.innerHTML;
    } catch (e) {
      console.error('Sanitize error:', e);
      return `<div class="render-error">Sanitization failed: ${escapeHtml(e.message)}</div>` + escapeHtml(dirtyHtml);
    }
  }

  // --- Render File Pipeline ---
  async function renderFile(fileData, preserveScroll = false) {
    if (el.dropOverlay) el.dropOverlay.classList.add('hidden');
    if (typeof window.__resetDragCounter === 'function') window.__resetDragCounter();

    logJS("renderFile called for: " + (fileData ? fileData.filename : "null") + ", path=" + (fileData ? fileData.path : "null"));
    if (!fileData || !fileData.success) {
      showError(fileData ? fileData.error : 'Failed to load file');
      return;
    }

    headingSlugCounts.clear();

    state.currentFile = fileData.path;
    state.rawContent = fileData.raw_content || fileData.content || '';
    state.stats = fileData.stats || {};

    // 1. Update Header Breadcrumbs
    const dirDisplay = fileData.dirname || fileData.dirpath || '';
    el.breadcrumbDir.textContent = dirDisplay;
    el.breadcrumbSeparator.style.display = dirDisplay ? 'inline' : 'none';
    el.breadcrumbFile.textContent = fileData.filename || 'Untitled.md';

    // 2. Hide Welcome, Show Article
    el.welcomeScreen.style.display = 'none';
    el.markdownArticle.style.display = 'block';
    document.title = `${fileData.filename || 'Document'} - QuickMD`;
    logJS("renderFile: welcome hidden, article displayed");

    // 3. Render Frontmatter Card if present
    if (fileData.frontmatter) {
      renderFrontmatter(fileData.frontmatter);
    } else {
      el.frontmatterContainer.style.display = 'none';
      el.frontmatterContainer.innerHTML = '';
    }

    // 4. Parse Markdown into HTML with XSS sanitization
    const preprocessed = preprocessMarkdown(fileData.content);
    const rawParsed = marked.parse(preprocessed);
    el.articleBody.innerHTML = sanitizeMarkdownHtml(rawParsed);

    // 5. Post-render: KaTeX Math Equations
    if (window.renderMathInElement) {
      try {
        window.renderMathInElement(el.articleBody, {
          delimiters: [
            { left: '$$', right: '$$', display: true },
            { left: '$', right: '$', display: false },
            { left: '\\(', right: '\\)', display: false },
            { left: '\\[', right: '\\]', display: true },
          ],
          throwOnError: false,
        });
      } catch (kErr) {
        console.warn('KaTeX render error:', kErr);
      }
    }

    // 6. Post-render: Mermaid Diagrams (On-demand lazy load)
    try {
      const mermaidNodes = el.articleBody.querySelectorAll('.mermaid');
      if (mermaidNodes.length > 0) {
        const mermaidInstance = await ensureMermaidLoaded();
        if (mermaidInstance) {
          await mermaidInstance.run({ nodes: mermaidNodes });
        }
      }
    } catch (mErr) {
      console.warn('Mermaid render error:', mErr);
    }

    // 7. Post-render: Build Table of Contents (Outline)
    try {
      buildOutlineTOC();
    } catch (tocErr) {
      console.warn('TOC error:', tocErr);
    }

    // 8. Post-render: Image Lightbox triggers
    try {
      setupImageLightboxes();
    } catch (imgErr) {
      console.warn('Lightbox error:', imgErr);
    }

    // 9. Update Status Bar & Stats
    try {
      updateStatusBar(fileData);
      renderRecentFiles(fileData.recent_files || state.recentFiles);
    } catch (statErr) {
      console.warn('Status error:', statErr);
    }

    // 10. Refresh Lucide Icons in newly injected DOM
    try {
      updateIcons();
    } catch (icoErr) {
      console.warn('Lucide error:', icoErr);
    }

    // Reset scroll to top
    if (el.contentScroller && !preserveScroll) {
      el.contentScroller.scrollTop = 0;
    }
  }

  // --- Frontmatter YAML Renderer ---
  function renderFrontmatter(yamlStr) {
    try {
      if (!window.jsyaml) return;
      const parsed = window.jsyaml.load(yamlStr);
      if (parsed && typeof parsed === 'object') {
        let rowsHtml = '';
        for (const [key, val] of Object.entries(parsed)) {
          let valDisplay = typeof val === 'object' ? JSON.stringify(val) : String(val);
          rowsHtml += `
            <div class="frontmatter-row">
              <span class="frontmatter-key">${escapeHtml(key)}:</span>
              <span class="frontmatter-val">${escapeHtml(valDisplay)}</span>
            </div>
          `;
        }
        el.frontmatterContainer.innerHTML = `
          <div class="frontmatter-card">
            <div class="frontmatter-header">
              <i data-lucide="tag" style="width: 12px; height: 12px;"></i>
              <span>Document Metadata (Frontmatter)</span>
            </div>
            ${rowsHtml}
          </div>
        `;
        el.frontmatterContainer.style.display = 'block';
      }
    } catch (e) {
      console.warn('Frontmatter parse error:', e);
      el.frontmatterContainer.style.display = 'none';
    }
  }

  // --- Outline (Table of Contents) & ScrollSpy ---
  function buildOutlineTOC() {
    el.tocContainer.innerHTML = '';
    if (el.contentScroller && el.contentScroller._scrollSpyHandler) {
      el.contentScroller.removeEventListener('scroll', el.contentScroller._scrollSpyHandler);
      el.contentScroller._scrollSpyHandler = null;
    }
    if (el.readingProgressBar) {
      el.readingProgressBar.style.width = '0%';
    }
    const headings = el.articleBody.querySelectorAll('h1, h2, h3, h4, h5, h6');

    el.outlineCount.textContent = `${headings.length} Heading${headings.length === 1 ? '' : 's'}`;

    if (headings.length === 0) {
      el.tocContainer.innerHTML = '<li class="toc-item" style="padding: 10px; font-size: 12px; color: var(--text-muted);">No headings found</li>';
      return;
    }

    headings.forEach((heading) => {
      const level = parseInt(heading.tagName.substring(1), 10);
      const text = heading.innerText.replace(/^#\s*/, '').trim();
      const id = heading.id;

      const li = document.createElement('li');
      li.className = 'toc-item';

      const a = document.createElement('a');
      a.className = `toc-link toc-level-${level}`;
      a.href = `#${id}`;
      a.textContent = text;
      a.title = text;

      a.addEventListener('click', (e) => {
        e.preventDefault();
        heading.scrollIntoView({ behavior: 'smooth', block: 'start' });
      });

      li.appendChild(a);
      el.tocContainer.appendChild(li);
    });

    setupScrollSpy(headings);
  }

  function setupScrollSpy(headings) {
    if (!el.contentScroller) return;

    if (el.contentScroller._scrollSpyHandler) {
      el.contentScroller.removeEventListener('scroll', el.contentScroller._scrollSpyHandler);
      el.contentScroller._scrollSpyHandler = null;
    }

    let ticking = false;
    const handleScroll = () => {
      if (!ticking) {
        window.requestAnimationFrame(() => {
          if (!el.contentScroller || !el.tocContainer) {
            ticking = false;
            return;
          }
          const scrollPos = el.contentScroller.scrollTop + 60;
          let currentActiveId = null;

          headings.forEach((heading) => {
            if (heading.offsetTop <= scrollPos) {
              currentActiveId = heading.id;
            }
          });

          const tocLinks = el.tocContainer.querySelectorAll('.toc-link');
          tocLinks.forEach((link) => {
            if (currentActiveId && link.getAttribute('href') === `#${currentActiveId}`) {
              link.classList.add('active');
            } else {
              link.classList.remove('active');
            }
          });

          // Update Reading Progress Bar
          const totalScroll = el.contentScroller.scrollHeight - el.contentScroller.clientHeight;
          if (totalScroll > 0) {
            const progress = Math.min(100, Math.max(0, (el.contentScroller.scrollTop / totalScroll) * 100));
            if (el.readingProgressBar) {
              el.readingProgressBar.style.width = `${progress}%`;
            }
          }
          ticking = false;
        });
        ticking = true;
      }
    };

    el.contentScroller._scrollSpyHandler = handleScroll;
    el.contentScroller.addEventListener('scroll', handleScroll, { passive: true });
  }

  // --- Image Lightbox Handler ---
  function setupImageLightboxes() {
    if (!el.articleBody || !el.lightboxImg || !el.lightboxOverlay) return;
    const images = el.articleBody.querySelectorAll('img');
    images.forEach((img) => {
      img.addEventListener('click', () => {
        if (!img.complete || img.naturalWidth === 0) {
          showToast('Image could not be loaded');
          return;
        }
        el.lightboxImg.src = img.src;
        el.lightboxOverlay.classList.remove('hidden');
      });
    });
  }

  // --- Status Bar Updates ---
  function updateStatusBar(fileData) {
    const s = (fileData && fileData.stats) ? fileData.stats : {};
    if (el.statusWords) el.statusWords.textContent = `${(s.words || 0).toLocaleString()} words`;
    if (el.statusReadtime) el.statusReadtime.textContent = `${s.reading_time_minutes || 0} min read`;
    if (el.statusSize) el.statusSize.textContent = fileData.size_formatted || '';
    if (el.statusEncoding) el.statusEncoding.textContent = fileData.encoding ? fileData.encoding.toUpperCase() : 'UTF-8';
    if (el.statusMtime) el.statusMtime.textContent = fileData.last_modified ? `Modified: ${fileData.last_modified}` : '';

    // Populate Stats Modal
    if (el.statWords) el.statWords.textContent = (s.words || 0).toLocaleString();
    if (el.statReadtime) el.statReadtime.textContent = `${s.reading_time_minutes || 0} min`;
    if (el.statChars) el.statChars.textContent = (s.chars || 0).toLocaleString();
    if (el.statLines) el.statLines.textContent = (s.lines || 0).toLocaleString();
    if (el.statHeadings) el.statHeadings.textContent = (s.headings || 0).toLocaleString();
    if (el.statCodeblocks) el.statCodeblocks.textContent = (s.code_blocks || 0).toLocaleString();
  }

  // --- Recent Files Sidebar Renderer ---
  function renderRecentFiles(recentList) {
    state.recentFiles = recentList || [];
    el.recentFilesList.innerHTML = '';

    if (!recentList || recentList.length === 0) {
      el.recentFilesList.innerHTML = '<div style="font-size: 11px; color: var(--text-muted); padding: 4px 10px;">No recent files</div>';
      return;
    }

    recentList.forEach((item) => {
      const filePath = typeof item === 'string' ? item : (item && item.path ? item.path : '');
      if (!filePath) return;
      const cleanPath = filePath.replace(/[\\/]+$/, '');
      const filename = typeof item === 'object' && item.filename ? item.filename : (cleanPath.split(/[\\/]/).pop() || 'Untitled.md');
      const div = document.createElement('div');
      div.className = 'recent-file-item';
      div.innerHTML = `
        <svg data-lucide="file-text"></svg>
        <span title="${escapeHtml(filePath)}">${escapeHtml(filename)}</span>
      `;

      div.addEventListener('click', async () => {
        if (window.pywebview && window.pywebview.api) {
          const res = await window.pywebview.api.load_file(filePath);
          if (res && res.success) {
            renderFile(res);
          }
        }
      });

      el.recentFilesList.appendChild(div);
    });

    updateIcons();
  }

  // --- In-Page Search (Ctrl+F) ---
  function openSearch() {
    el.searchModal.classList.remove('hidden');
    window.requestAnimationFrame(() => {
      setTimeout(() => {
        if (el.searchInput) {
          el.searchInput.focus();
          el.searchInput.select();
        }
      }, 30);
    });
  }

  function closeSearch() {
    el.searchModal.classList.add('hidden');
    clearSearchHighlights();
  }

  function performSearch() {
    clearSearchHighlights();
    const query = el.searchInput.value.trim();
    if (!query) {
      el.searchCount.textContent = '0/0';
      return;
    }

    const regex = new RegExp(`(${escapeRegex(query)})`, 'gi');
    highlightTextInNode(el.articleBody, regex);

    state.searchMatches = Array.from(el.articleBody.querySelectorAll('.search-highlight'));
    state.currentMatchIndex = state.searchMatches.length > 0 ? 0 : -1;

    updateSearchMatchDisplay();
  }

  function highlightTextInNode(node, regex) {
    if (node.nodeType === Node.TEXT_NODE) {
      const text = node.nodeValue;
      if (!text || !regex.test(text)) return;
      regex.lastIndex = 0;

      const fragment = document.createDocumentFragment();
      let lastIdx = 0;
      let match;
      while ((match = regex.exec(text)) !== null) {
        if (match.index > lastIdx) {
          fragment.appendChild(document.createTextNode(text.substring(lastIdx, match.index)));
        }
        const mark = document.createElement('span');
        mark.className = 'search-highlight';
        mark.textContent = match[0];
        fragment.appendChild(mark);
        lastIdx = regex.lastIndex;
        if (match[0].length === 0) {
          regex.lastIndex++;
        }
      }
      if (lastIdx < text.length) {
        fragment.appendChild(document.createTextNode(text.substring(lastIdx)));
      }
      node.parentNode.replaceChild(fragment, node);
    } else if (
      node.nodeType === Node.ELEMENT_NODE &&
      !['SCRIPT', 'STYLE', 'NOSCRIPT', 'SVG', 'MATH'].includes(node.tagName) &&
      !node.classList.contains('katex') &&
      !node.classList.contains('katex-display') &&
      !node.classList.contains('katex-html') &&
      !node.classList.contains('katex-mathml')
    ) {
      Array.from(node.childNodes).forEach((child) => highlightTextInNode(child, regex));
    }
  }

  function clearSearchHighlights() {
    const highlights = el.articleBody.querySelectorAll('.search-highlight');
    highlights.forEach((h) => {
      const parent = h.parentNode;
      parent.replaceChild(document.createTextNode(h.textContent), h);
      parent.normalize();
    });
    state.searchMatches = [];
    state.currentMatchIndex = -1;
  }

  function updateSearchMatchDisplay() {
    state.searchMatches.forEach((m, idx) => {
      if (idx === state.currentMatchIndex) {
        m.classList.add('active-match');
        m.scrollIntoView({ behavior: 'smooth', block: 'center' });
      } else {
        m.classList.remove('active-match');
      }
    });

    if (state.searchMatches.length > 0) {
      el.searchCount.textContent = `${state.currentMatchIndex + 1}/${state.searchMatches.length}`;
    } else {
      el.searchCount.textContent = '0/0';
    }
  }

  function nextSearchMatch() {
    if (state.searchMatches.length === 0) return;
    state.currentMatchIndex = (state.currentMatchIndex + 1) % state.searchMatches.length;
    updateSearchMatchDisplay();
  }

  function prevSearchMatch() {
    if (state.searchMatches.length === 0) return;
    state.currentMatchIndex = (state.currentMatchIndex - 1 + state.searchMatches.length) % state.searchMatches.length;
    updateSearchMatchDisplay();
  }

  // --- Theme & Appearance Settings ---
  function applySettings(cfg) {
    if (!cfg) return;

    if (cfg.theme) {
      setTheme(cfg.theme, false);
    }
    if (cfg.content_width) {
      setContentWidth(cfg.content_width, false);
    }
    if (cfg.font_size) {
      setFontSize(cfg.font_size, false);
    }
    if (cfg.sidebar_open !== undefined) {
      toggleSidebar(cfg.sidebar_open, false);
    }
  }

  function setTheme(themeName, persist = true) {
    state.theme = themeName;
    THEMES.forEach((t) => document.body.classList.remove(`theme-${t}`));
    document.body.classList.add(`theme-${themeName}`);

    // Update Highlight.js stylesheet
    if (themeName === 'light' || themeName === 'sepia') {
      el.highlightThemeLink.href = '../css/highlight-light.min.css';
    } else {
      el.highlightThemeLink.href = '../css/highlight-dark.min.css';
    }

    // Dynamic Mermaid theme update
    if (el.articleBody) {
      const containers = el.articleBody.querySelectorAll('.mermaid-container[data-mermaid-raw]');
      if (containers.length > 0) {
        ensureMermaidLoaded().then((mermaidInstance) => {
          if (!mermaidInstance) return;
          try {
            const isLight = themeName === 'light' || themeName === 'sepia';
            mermaidInstance.initialize({
              startOnLoad: false,
              theme: isLight ? 'default' : 'dark',
              securityLevel: 'strict',
              fontFamily: 'var(--font-sans)',
            });
            const newNodes = [];
            containers.forEach((c) => {
              const raw = decodeURIComponent(c.getAttribute('data-mermaid-raw') || '');
              if (raw) {
                c.innerHTML = `<pre class="mermaid">${escapeHtml(raw)}</pre>`;
                const pre = c.querySelector('.mermaid');
                if (pre) newNodes.push(pre);
              }
            });
            if (newNodes.length > 0) {
              mermaidInstance.run({ nodes: newNodes }).catch((e) => console.warn('Mermaid theme re-render error:', e));
            }
          } catch (mErr) {
            console.warn('Mermaid theme re-init error:', mErr);
          }
        });
      }
    }

    if (persist && window.pywebview && window.pywebview.api) {
      window.pywebview.api.save_settings({ theme: themeName });
    }
  }

  function cycleTheme() {
    const currentIndex = THEMES.indexOf(state.theme);
    const nextIndex = (currentIndex + 1) % THEMES.length;
    setTheme(THEMES[nextIndex]);
  }

  function setContentWidth(widthName, persist = true) {
    state.contentWidth = widthName;
    WIDTHS.forEach((w) => document.body.classList.remove(`width-${w}`));
    document.body.classList.add(`width-${widthName}`);

    if (persist && window.pywebview && window.pywebview.api) {
      window.pywebview.api.save_settings({ content_width: widthName });
    }
  }

  function cycleContentWidth() {
    const currentIndex = WIDTHS.indexOf(state.contentWidth);
    const nextIndex = (currentIndex + 1) % WIDTHS.length;
    setContentWidth(WIDTHS[nextIndex]);
  }

  function setFontSize(sizePx, persist = true) {
    const clamped = Math.min(28, Math.max(12, sizePx));
    state.fontSize = clamped;
    document.documentElement.style.setProperty('--reading-font-size', `${clamped}px`);
    const zoomPct = Math.round((clamped / 16) * 100);
    el.zoomLabel.textContent = `${zoomPct}%`;

    if (persist && window.pywebview && window.pywebview.api) {
      window.pywebview.api.save_settings({ font_size: clamped });
    }
  }

  function toggleSidebar(forceState = null, persist = true) {
    if (forceState !== null) {
      state.sidebarOpen = forceState;
    } else {
      state.sidebarOpen = !state.sidebarOpen;
    }

    if (state.sidebarOpen) {
      el.sidebar.classList.remove('hidden');
      el.btnToggleSidebar.classList.add('active');
    } else {
      el.sidebar.classList.add('hidden');
      el.btnToggleSidebar.classList.remove('active');
    }

    if (persist && window.pywebview && window.pywebview.api) {
      window.pywebview.api.save_settings({ sidebar_open: state.sidebarOpen });
    }
  }

  // Calculate stats in fallback mode for full fidelity
  function calculateClientStats(content) {
    if (!content) return {};
    const lines = content.split('\n').length;
    const chars = content.length;
    const headings = (content.match(/^#{1,6}\s+/gm) || []).length;
    const codeBlocks = (content.match(/^(?:`{3,}|~{3,})/gm) || []).length / 2;
    const words = (content.match(/[\w\u0E00-\u0E7F]+/g) || []).length;
    const readTime = Math.max(1, Math.ceil(words / 200));
    return {
      words,
      lines,
      chars,
      headings,
      code_blocks: Math.floor(codeBlocks),
      reading_time_minutes: readTime,
    };
  }

  // --- Event Listeners Setup ---
  function setupEventListeners() {
    // Toolbar Button Actions
    if (el.btnOpenFile) el.btnOpenFile.addEventListener('click', handleOpenFile);
    if (el.btnWelcomeOpen) el.btnWelcomeOpen.addEventListener('click', handleOpenFile);
    if (el.dropzone) el.dropzone.addEventListener('click', handleOpenFile);
    if (el.btnToggleSidebar) el.btnToggleSidebar.addEventListener('click', () => toggleSidebar());
    if (el.btnSearch) el.btnSearch.addEventListener('click', openSearch);
    if (el.btnStats && el.statsModalOverlay) {
      el.btnStats.addEventListener('click', () => el.statsModalOverlay.classList.remove('hidden'));
    }
    if (el.btnStatsClose && el.statsModalOverlay) {
      el.btnStatsClose.addEventListener('click', () => el.statsModalOverlay.classList.add('hidden'));
    }
    if (el.statsModalOverlay) {
      el.statsModalOverlay.addEventListener('click', (e) => {
        if (e.target === el.statsModalOverlay) el.statsModalOverlay.classList.add('hidden');
      });
    }

    if (el.btnInfo && el.aboutModalOverlay) {
      el.btnInfo.addEventListener('click', () => {
        el.aboutModalOverlay.classList.remove('hidden');
        if (window.lucide) window.lucide.createIcons();
      });
    }
    if (el.btnAboutClose && el.aboutModalOverlay) {
      el.btnAboutClose.addEventListener('click', () => el.aboutModalOverlay.classList.add('hidden'));
    }
    if (el.aboutModalOverlay) {
      el.aboutModalOverlay.addEventListener('click', (e) => {
        if (e.target === el.aboutModalOverlay) el.aboutModalOverlay.classList.add('hidden');
      });
    }

    if (el.btnTheme) el.btnTheme.addEventListener('click', cycleTheme);
    if (el.btnWidth) el.btnWidth.addEventListener('click', cycleContentWidth);
    if (el.btnZoomIn) el.btnZoomIn.addEventListener('click', () => setFontSize(state.fontSize + 1));
    if (el.btnZoomOut) el.btnZoomOut.addEventListener('click', () => setFontSize(state.fontSize - 1));
    if (el.btnZoomReset) el.btnZoomReset.addEventListener('click', () => setFontSize(16));
    if (el.btnPrint) el.btnPrint.addEventListener('click', () => window.print());

    if (el.btnCopyRaw) {
      el.btnCopyRaw.addEventListener('click', async () => {
        if (state.rawContent && window.pywebview && window.pywebview.api) {
          await window.pywebview.api.copy_to_clipboard(state.rawContent);
          showToast('Raw Markdown copied to clipboard!');
        }
      });
    }

    // Windows Integration Registration
    if (el.btnWindowsAssoc) {
      el.btnWindowsAssoc.addEventListener('click', handleRegisterWindows);
    }
    if (el.btnSetupWindows) {
      el.btnSetupWindows.addEventListener('click', handleRegisterWindows);
    }

    // Fallback File Input Change Listener
    const fallbackInput = document.getElementById('fallback-file-input');
    if (fallbackInput) {
      fallbackInput.addEventListener('change', (e) => {
        if (e.target.files && e.target.files.length > 0) {
          const file = e.target.files[0];
          const reader = new FileReader();
          reader.onload = (evt) => {
            const content = evt.target.result;
            const computedStats = calculateClientStats(content);
            renderFile({
              success: true,
              filename: file.name,
              path: file.name,
              dirname: '',
              dirpath: '',
              content: content,
              raw_content: content,
              size_formatted: `${(file.size / 1024).toFixed(1)} KB`,
              size_kb: Math.round(file.size / 102.4) / 10.0,
              encoding: 'UTF-8',
              last_modified: 'Just now',
              mtime: 'Just now',
              stats: computedStats,
              recent_files: state.recentFiles || [],
            });
            showToast(`Opened: ${file.name} ✓`);
          };
          reader.readAsText(file);
        }
      });
    }

    // Expose renderFile to global window for native Rust event invocation
    window.renderFile = renderFile;

    // Lightbox Overlay Dismiss on Click
    if (el.lightboxOverlay) {
      el.lightboxOverlay.addEventListener('click', () => {
        el.lightboxOverlay.classList.add('hidden');
      });
    }

    // Global Link, Wikilink & Action Interceptor
    document.addEventListener('click', (e) => {
      // 0. Code & Table Copy Buttons (Delegation to bypass CSP & sanitizer)
      const codeCopyBtn = e.target.closest('.code-copy-btn');
      if (codeCopyBtn) {
        e.preventDefault();
        if (window.copyCodeFromBlock) {
          window.copyCodeFromBlock(codeCopyBtn);
        }
        return;
      }

      const tableCopyBtn = e.target.closest('.copy-table-btn');
      if (tableCopyBtn) {
        e.preventDefault();
        if (window.copyTableContent) {
          window.copyTableContent(tableCopyBtn);
        }
        return;
      }

      // 1. Wikilink Delegation (data-wikilink & data-anchor)
      const wikilink = e.target.closest('a.wikilink');
      if (wikilink) {
        e.preventDefault();
        const target = wikilink.getAttribute('data-wikilink') || '';
        const anchor = wikilink.getAttribute('data-anchor') || '';
        if (window.handleWikilink) {
          window.handleWikilink(target, anchor);
        }
        return;
      }

      // 2. External & Relative Links Interception
      const a = e.target.closest('a');
      if (a) {
        const rawHref = (a.getAttribute('href') || '').trim();
        if (a.classList.contains('toc-link') || a.classList.contains('wikilink')) {
          return;
        }

        // In-page hash anchors: #heading-id, #slug, or heading-anchor
        if (rawHref.startsWith('#')) {
          e.preventDefault();
          scrollToHeadingAnchor(rawHref);
          return;
        }

        // Auto-handle www. links
        if (rawHref.startsWith('www.')) {
          e.preventDefault();
          const fullUrl = `https://${rawHref}`;
          if (window.pywebview && window.pywebview.api) {
            window.pywebview.api.open_external_url(fullUrl);
          } else {
            window.open(fullUrl, '_blank');
          }
          return;
        }

        // External URLs (http, https, mailto, or any URI scheme)
        if (rawHref.startsWith('http://') || rawHref.startsWith('https://') || rawHref.startsWith('mailto:') || /^[a-z][a-z0-9+.-]*:/i.test(rawHref)) {
          e.preventDefault();
          if (window.pywebview && window.pywebview.api) {
            window.pywebview.api.open_external_url(rawHref);
          } else {
            window.open(rawHref, '_blank');
          }
          return;
        }

        // Relative Markdown / Document link (e.g. chapter2.md, ./sub/doc.markdown)
        e.preventDefault();
        const parts = rawHref.split('#');
        const targetDoc = parts[0].split('?')[0].trim();
        const anchor = parts[1] || null;
        if (targetDoc.endsWith('.md') || targetDoc.endsWith('.markdown') || targetDoc.endsWith('.mdown') || targetDoc.endsWith('.mkdn') || targetDoc.endsWith('.txt') || !targetDoc.includes('.')) {
          if (window.handleWikilink && targetDoc.length > 0) {
            window.handleWikilink(targetDoc, anchor);
          }
        } else {
          showToast(`Cannot open non-markdown file: ${targetDoc}`);
        }
      }
    });

    // In-Page Search Listeners
    if (el.searchInput) {
      el.searchInput.addEventListener('input', performSearch);
      el.searchInput.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') {
          if (e.shiftKey) prevSearchMatch();
          else nextSearchMatch();
        } else if (e.key === 'Escape') {
          closeSearch();
        }
      });
    }
    if (el.btnSearchNext) el.btnSearchNext.addEventListener('click', nextSearchMatch);
    if (el.btnSearchPrev) el.btnSearchPrev.addEventListener('click', prevSearchMatch);
    if (el.btnSearchClose) el.btnSearchClose.addEventListener('click', closeSearch);

    // Full-Window Drag and Drop Handling
    let dragCounter = 0;
    window.__resetDragCounter = () => {
      dragCounter = 0;
    };
    window.addEventListener('dragenter', (e) => {
      e.preventDefault();
      e.stopPropagation();
      dragCounter++;
      if (el.dropOverlay) el.dropOverlay.classList.remove('hidden');
    });

    window.addEventListener('dragover', (e) => {
      e.preventDefault();
      e.stopPropagation();
    });

    window.addEventListener('dragleave', (e) => {
      e.preventDefault();
      e.stopPropagation();
      dragCounter--;
      if (dragCounter <= 0) {
        dragCounter = 0;
        if (el.dropOverlay) el.dropOverlay.classList.add('hidden');
      }
    });

    window.addEventListener('drop', async (e) => {
      e.preventDefault();
      e.stopPropagation();
      dragCounter = 0;
      if (el.dropOverlay) el.dropOverlay.classList.add('hidden');

      if (e.dataTransfer && e.dataTransfer.files.length > 0) {
        const totalCount = e.dataTransfer.files.length;
        const file = e.dataTransfer.files[0];
        if (totalCount > 1) {
          showToast(`เปิดไฟล์แรกแล้ว (ลากมา ${totalCount} ไฟล์)`);
        }
        if (file.path && window.pywebview && window.pywebview.api) {
          const res = await window.pywebview.api.load_file(file.path);
          if (res && res.success) renderFile(res);
        } else {
          // WebView2 sandbox fallback: read via FileReader
          const reader = new FileReader();
          reader.onload = (evt) => {
            const content = evt.target.result;
            renderFile({
              success: true,
              filename: file.name,
              path: file.name,
              dirname: '',
              dirpath: '',
              content: content,
              raw_content: content,
              size_formatted: `${(file.size / 1024).toFixed(1)} KB`,
              size_kb: Math.round(file.size / 102.4) / 10.0,
              encoding: 'UTF-8',
              last_modified: 'Just now',
              mtime: 'Just now',
              stats: {},
              recent_files: state.recentFiles || [],
            });
            showToast(`Opened: ${file.name} ✓`);
          };
          reader.readAsText(file);
        }
      }
    });

    // Global Keyboard Shortcuts
    window.addEventListener('keydown', async (e) => {
      // Ctrl + O: Open File
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'o') {
        e.preventDefault();
        handleOpenFile();
      }
      // Ctrl + F: Search
      else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') {
        e.preventDefault();
        openSearch();
      }
      // Ctrl + T: Toggle Outline
      else if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === 't') {
        e.preventDefault();
        toggleSidebar();
      }
      // Ctrl + Shift + T: Cycle Theme
      else if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 't') {
        e.preventDefault();
        cycleTheme();
      }
      // Ctrl + I: Document Stats
      else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'i') {
        e.preventDefault();
        el.statsModalOverlay.classList.toggle('hidden');
      }
      // Ctrl + P: Print / PDF
      else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'p') {
        e.preventDefault();
        window.print();
      }
      // Ctrl + = / Ctrl + +: Zoom In
      else if ((e.ctrlKey || e.metaKey) && (e.key === '=' || e.key === '+')) {
        e.preventDefault();
        setFontSize(state.fontSize + 1);
      }
      // Ctrl + -: Zoom Out
      else if ((e.ctrlKey || e.metaKey) && e.key === '-') {
        e.preventDefault();
        setFontSize(state.fontSize - 1);
      }
      // Ctrl + 0: Zoom Reset
      else if ((e.ctrlKey || e.metaKey) && e.key === '0') {
        e.preventDefault();
        setFontSize(16);
      }
      // Esc: Close Modals
      else if (e.key === 'Escape') {
        closeSearch();
        el.statsModalOverlay.classList.add('hidden');
        if (el.aboutModalOverlay) el.aboutModalOverlay.classList.add('hidden');
        el.lightboxOverlay.classList.add('hidden');
      }
      // Ctrl + Q / Ctrl + W: Close Window cleanly
      else if ((e.ctrlKey || e.metaKey) && (e.key.toLowerCase() === 'q' || e.key.toLowerCase() === 'w')) {
        e.preventDefault();
        if (window.pywebview && window.pywebview.api) {
          window.pywebview.api.close_app();
        } else {
          window.close();
        }
      }
    });

    // Ctrl + Mouse Wheel dynamic font zooming
    window.addEventListener('wheel', (e) => {
      if (e.ctrlKey) {
        e.preventDefault();
        if (e.deltaY < 0) {
          setFontSize(state.fontSize + 1);
        } else if (e.deltaY > 0) {
          setFontSize(state.fontSize - 1);
        }
      }
    }, { passive: false });
  }

  async function handleOpenFile() {
    logJS("handleOpenFile invoked");
    if (window.pywebview && window.pywebview.api) {
      try {
        logJS("Invoking native file dialog...");
        const res = await window.pywebview.api.open_file_dialog();
        if (res && res.success) {
          await renderFile(res);
          showToast(`Opened: ${res.filename} ✓`);
        } else if (res && res.error) {
          showError(res.error);
        }
      } catch (err) {
        logJS("handleOpenFile failed: " + err);
        showError("Failed to open file: " + err);
        const fallbackInput = document.getElementById('fallback-file-input');
        if (fallbackInput) fallbackInput.click();
      }
    } else {
      const fallbackInput = document.getElementById('fallback-file-input');
      if (fallbackInput) fallbackInput.click();
    }
  }

  async function handleRegisterWindows() {
    if (window.pywebview && window.pywebview.api) {
      const res = await window.pywebview.api.register_windows_integration();
      if (res && res.success) {
        showToast('Registered as Windows Default Markdown Reader! ✓');
        if (el.windowsAssocBanner) {
          el.windowsAssocBanner.style.display = 'none';
        }
        if (el.btnWindowsAssoc) {
          el.btnWindowsAssoc.style.display = 'none';
        }
      } else {
        showError(res ? res.error : 'Registration failed');
      }
    }
  }

  // --- Helper Functions ---
  window.copyCodeFromBlock = async function (btn) {
    const container = btn.closest('.code-block-container');
    if (!container) return;
    const codeEl = container.querySelector('code');
    if (!codeEl) return;

    const codeText = codeEl.innerText;
    if (window.pywebview && window.pywebview.api) {
      await window.pywebview.api.copy_to_clipboard(codeText);
    } else {
      navigator.clipboard.writeText(codeText);
    }

    btn.classList.add('copied');
    btn.innerHTML = '<span>Copied! ✓</span>';
    setTimeout(() => {
      btn.classList.remove('copied');
      btn.innerHTML = '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg><span>Copy</span>';
    }, 2000);
  };

  window.copyTableContent = async function (btn) {
    const wrapper = btn.closest('.table-wrapper');
    if (!wrapper) return;
    const table = wrapper.querySelector('table');
    if (!table) return;

    let md = '';
    const thead = table.querySelector('thead');
    if (thead) {
      const headerRow = thead.querySelector('tr');
      if (headerRow) {
        const headers = Array.from(headerRow.querySelectorAll('th, td')).map((c) => c.innerText.trim().replace(/\|/g, '\\|'));
        if (headers.length > 0) {
          md += '| ' + headers.join(' | ') + ' |\n';
          md += '| ' + headers.map(() => '---').join(' | ') + ' |\n';
        }
      }
      const bodyRows = table.querySelectorAll('tbody tr');
      bodyRows.forEach((row) => {
        const cols = Array.from(row.querySelectorAll('td, th')).map((c) => c.innerText.trim().replace(/\|/g, '\\|'));
        if (cols.length > 0) {
          md += '| ' + cols.join(' | ') + ' |\n';
        }
      });
    } else {
      // Headless table: use first row as header
      const allRows = Array.from(table.querySelectorAll('tr'));
      if (allRows.length > 0) {
        const firstCols = Array.from(allRows[0].querySelectorAll('td, th')).map((c) => c.innerText.trim().replace(/\|/g, '\\|'));
        if (firstCols.length > 0) {
          md += '| ' + firstCols.join(' | ') + ' |\n';
          md += '| ' + firstCols.map(() => '---').join(' | ') + ' |\n';
        }
        allRows.slice(1).forEach((row) => {
          const cols = Array.from(row.querySelectorAll('td, th')).map((c) => c.innerText.trim().replace(/\|/g, '\\|'));
          if (cols.length > 0) {
            md += '| ' + cols.join(' | ') + ' |\n';
          }
        });
      }
    }

    const exportText = md || table.innerText;

    if (window.pywebview && window.pywebview.api) {
      await window.pywebview.api.copy_to_clipboard(exportText);
    } else {
      navigator.clipboard.writeText(exportText);
    }

    btn.innerHTML = '<span>Copied! ✓</span>';
    setTimeout(() => {
      btn.innerHTML = '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"></path><rect x="8" y="2" width="8" height="4" rx="1" ry="1"></rect></svg><span>Copy Table</span>';
    }, 2000);
  };

  function scrollToHeadingAnchor(anchorText) {
    if (!anchorText || !el.articleBody) return;
    let decoded = anchorText;
    try {
      decoded = decodeURIComponent(anchorText);
    } catch (_) {}
    decoded = decoded.replace(/^#+/, '').trim();
    if (!decoded) return;

    const cleanSlug = decoded.toLowerCase().replace(/[^\w\u0E00-\u0E7F]+/g, '-').replace(/^-+|-+$/g, '');
    const rawLower = decoded.toLowerCase();

    const elTarget = document.getElementById(decoded) ||
                     document.getElementById(cleanSlug) ||
                     document.getElementById(encodeURIComponent(decoded)) ||
                     document.getElementById(encodeURIComponent(cleanSlug)) ||
                     Array.from(el.articleBody.querySelectorAll('h1, h2, h3, h4, h5, h6')).find((h) => {
                       const t = h.innerText.replace(/^#\s*/, '').trim().toLowerCase();
                       const hSlug = t.replace(/[^\w\u0E00-\u0E7F]+/g, '-').replace(/^-+|-+$/g, '');
                       return t === rawLower || hSlug === cleanSlug || t === cleanSlug || h.id === cleanSlug || h.id === decoded;
                     });

    if (elTarget) {
      elTarget.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }
  }

  window.handleWikilink = async function (target, anchor = '') {
    // 1. Same-page anchor jump: [[#Section]]
    if (!target && anchor) {
      scrollToHeadingAnchor(anchor);
      return;
    }

    // 2. Cross-document wikilink: [[Target#Anchor]]
    if (window.pywebview && window.pywebview.api && window.pywebview.api.load_wikilink) {
      try {
        const res = await window.pywebview.api.load_wikilink(target);
        if (res && res.success) {
          await renderFile(res);
          showToast(`Opened: ${res.filename} ✓`);
          if (anchor) {
            setTimeout(() => scrollToHeadingAnchor(anchor), 150);
          }
          return;
        } else if (res && res.error) {
          showToast(res.error);
          return;
        }
      } catch (e) {
        console.warn('Wikilink load error:', e);
      }
    }
    showToast(`Note not found: ${target}`);
  };

  // --- Live Auto-Reload Handler ---
  window.onFileReloaded = async function(fileData) {
    if (!fileData || !fileData.success) return;
    const currentScroll = el.contentScroller ? el.contentScroller.scrollTop : 0;
    await renderFile(fileData, true);
    if (el.contentScroller) {
      el.contentScroller.scrollTop = currentScroll;
    }
    showToast('File auto-reloaded');
  };

  function showToast(msg) {
    let toast = document.getElementById('app-toast');
    if (!toast) {
      toast = document.createElement('div');
      toast.id = 'app-toast';
      toast.style.cssText = `
        position: fixed;
        bottom: 40px;
        left: 50%;
        transform: translateX(-50%);
        background: var(--bg-tertiary);
        color: var(--text-primary);
        border: 1px solid var(--border-color);
        padding: 8px 18px;
        border-radius: 20px;
        font-size: 12px;
        font-weight: 500;
        box-shadow: var(--shadow-lg);
        z-index: 150;
        transition: opacity 0.2s ease;
      `;
      document.body.appendChild(toast);
    }
    toast.textContent = msg;
    toast.style.opacity = '1';
    clearTimeout(toast._timer);
    toast._timer = setTimeout(() => {
      toast.style.opacity = '0';
    }, 2500);
  }
  window.showToast = showToast;

  function showError(msg) {
    showToast(`Error: ${msg}`);
  }

  function escapeHtml(str) {
    if (!str) return '';
    return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#039;');
  }

  function escapeRegex(str) {
    return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }
})();
