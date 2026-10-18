<!-- ========================================================================
     File          : Chat.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The AI Chat app: conversation view with streamed replies, model
     selection, and the MCP server management panel.

     Dependencies:
       - svelte (>= 5), theme + app/ composites, chat/McpPanel,
         $lib/services/api

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { languageStore, t, translate } from '$lib/stores/i18n';
  import { IconBrain, IconSend, IconSparkles, IconTrash, IconHistory, IconPlus, IconCopy, IconCheck, IconRefresh, IconSquare, IconCpu } from '@tabler/icons-svelte';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import { Button } from '$lib/components/theme';
  import { PageLayout, ConfirmDialog, EmptyState } from '$lib/components/app';
  import McpPanel from '$lib/components/chat/McpPanel.svelte';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { cn } from '$lib/utils.js';

  interface Message {
    role: 'user' | 'assistant';
    content: string;
    timestamp: Date;
  }

  interface Conversation {
    id: string;
    title: string;
    messages: Message[];
    lastUpdated: Date;
    modelName: string;
  }

  let messages: Message[] = [];
  let input = '';
  let sending = false;
  let conversationId: string | undefined = undefined;
  let messagesContainer: HTMLDivElement;
  let modelName = 'gemma3:1b';
  let conversations: Conversation[] = [];
  let showHistory = false;
  let streamCancel: (() => void) | null = null;
  let copiedIndex: number | null = null;
  const dlg = createConfirmState();

  // Real Ollama reachability — the status badge used to be a hardcoded
  // "Online" that stayed green even when every message failed.
  let ollamaStatus: 'checking' | 'online' | 'offline' | 'nomodel' = 'checking';

  $: if (messagesContainer && messages.length > 0) {
    scrollToBottom();
  }

  // External agents (MCP): the management endpoint is admin-only, so a
  // successful probe doubles as the admin check — non-admins never see the
  // panel toggle. Probed lazily on mount, independent of Ollama state.
  let isAdmin = false;
  let showMcp = false;

  onMount(() => {
    loadConversations();
    checkOllama();
    checkMcpAdmin();
  });

  async function checkMcpAdmin() {
    try {
      const res: any = await api.mcpStatus();
      isAdmin = !!res?.success;
    } catch {
      isAdmin = false;
    }
  }

  async function checkOllama() {
    try {
      const res: any = await api.chatListModels();
      const running = !!res?.data?.ollama_running;
      const models: unknown[] = res?.data?.models ?? [];
      if (res?.data?.current) modelName = res.data.current;
      ollamaStatus = running ? (models.length > 0 ? 'online' : 'nomodel') : 'offline';
    } catch {
      ollamaStatus = 'offline';
    }
  }

  function loadConversations() {
    const saved = localStorage.getItem('ollama_conversations');
    if (saved) {
      try {
        const parsed = JSON.parse(saved);
        conversations = parsed.map((conv: any) => ({
          ...conv,
          lastUpdated: new Date(conv.lastUpdated),
          messages: conv.messages.map((msg: any) => ({
            ...msg,
            timestamp: new Date(msg.timestamp)
          }))
        }));
      } catch (error) {
        console.error('Failed to load conversations:', error);
      }
    }
  }

  function saveConversations() {
    localStorage.setItem('ollama_conversations', JSON.stringify(conversations));
  }

  function scrollToBottom() {
    requestAnimationFrame(() => {
      if (messagesContainer) {
        messagesContainer.scrollTop = messagesContainer.scrollHeight;
      }
    });
  }

  async function sendMessage() {
    const text = input.trim();
    if (!text || sending) return;

    input = '';
    streamReply(text);
  }

  function streamReply(text: string) {
    sending = true;

    const userMsg: Message = {
      role: 'user',
      content: text,
      timestamp: new Date(),
    };
    messages = [...messages, userMsg];

    const history = messages
      .filter(m => m !== userMsg)
      .map(m => ({ role: m.role, content: m.content }));

    const assistantMsg: Message = {
      role: 'assistant',
      content: '',
      timestamp: new Date(),
    };
    messages = [...messages, assistantMsg];
    const assistantIndex = messages.length - 1;
    let gotAnyChunk = false;

    streamCancel = api.chatSendStream(
      text,
      history,
      conversationId,
      $languageStore,
      (chunk) => {
        gotAnyChunk = true;
        messages[assistantIndex].content += chunk;
      },
      (done) => {
        conversationId = done.conversation_id;
        modelName = done.model;
        sending = false;
        streamCancel = null;
        saveCurrentConversation();
      },
      () => {
        sending = false;
        streamCancel = null;
        if (!gotAnyChunk) {
          messages[assistantIndex].content = translate('chat.error_fallback_reply');
        } else {
          saveCurrentConversation();
        }
      }
    );
  }

  function stopGenerating() {
    if (streamCancel) {
      streamCancel();
      streamCancel = null;
    }
    sending = false;
    saveCurrentConversation();
  }

  function regenerateLast() {
    if (sending) return;

    let lastUserIndex = -1;
    for (let i = messages.length - 1; i >= 0; i--) {
      if (messages[i].role === 'user') {
        lastUserIndex = i;
        break;
      }
    }
    if (lastUserIndex === -1) return;

    const text = messages[lastUserIndex].content;
    messages = messages.slice(0, lastUserIndex);
    streamReply(text);
  }

  async function copyMessage(index: number, content: string) {
    try {
      await navigator.clipboard.writeText(content);
      copiedIndex = index;
      setTimeout(() => {
        if (copiedIndex === index) copiedIndex = null;
      }, 1500);
    } catch {
      // Clipboard permission denied/unavailable — nothing more to do
    }
  }

  function escapeHtml(str: string): string {
    return str
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function isSafeUrl(url: string): boolean {
    return /^(https?:|mailto:|\/)/i.test(url.trim());
  }

  /**
   * Minimal, dependency-free markdown renderer for chat bubbles: fenced/inline
   * code, bold/italic, links, and lists. HTML is escaped before any markup is
   * introduced, and link targets are scheme-checked, so this is safe against
   * a model reply that happens to contain HTML or a `javascript:` URL.
   */
  function renderMessageContent(raw: string): string {
    const codeBlocks: string[] = [];
    let text = raw.replace(/```(\w*)\n?([\s\S]*?)```/g, (_match, _lang, code) => {
      const idx = codeBlocks.length;
      codeBlocks.push(`<pre class="hv-chat-codeblock"><code>${escapeHtml(code.trim())}</code></pre>`);
      return `@@HVCODEBLOCK${idx}@@`;
    });

    text = escapeHtml(text);

    text = text.replace(/`([^`\n]+)`/g, (_m, code) => `<code class="hv-chat-inline-code">${code}</code>`);

    text = text.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (_m, label, url) =>
      isSafeUrl(url) ? `<a href="${escapeHtml(url)}" target="_blank" rel="noopener noreferrer">${label}</a>` : label
    );

    text = text.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
    text = text.replace(/(?<!\*)\*([^*\n]+)\*(?!\*)/g, '<em>$1</em>');

    const lines = text.split('\n');
    const out: string[] = [];
    let listType: 'ul' | 'ol' | null = null;
    for (const line of lines) {
      const bulletMatch = /^[-*]\s+(.*)$/.exec(line);
      const numberedMatch = /^\d+\.\s+(.*)$/.exec(line);
      if (bulletMatch) {
        if (listType !== 'ul') {
          if (listType) out.push(`</${listType}>`);
          out.push('<ul>');
          listType = 'ul';
        }
        out.push(`<li>${bulletMatch[1]}</li>`);
      } else if (numberedMatch) {
        if (listType !== 'ol') {
          if (listType) out.push(`</${listType}>`);
          out.push('<ol>');
          listType = 'ol';
        }
        out.push(`<li>${numberedMatch[1]}</li>`);
      } else {
        if (listType) {
          out.push(`</${listType}>`);
          listType = null;
        }
        out.push(line);
      }
    }
    if (listType) out.push(`</${listType}>`);
    text = out.join('\n');

    text = text.replace(/\n/g, '<br>');
    text = text.replace(/@@HVCODEBLOCK(\d+)@@/g, (_m, idx) => codeBlocks[Number(idx)]);

    return text;
  }

  function saveCurrentConversation() {
    if (messages.length === 0) return;

    const title = messages[0].content.substring(0, 50) + (messages[0].content.length > 50 ? '...' : '');
    const existingIndex = conversations.findIndex(c => c.id === conversationId);

    const conversation: Conversation = {
      id: conversationId || crypto.randomUUID(),
      title,
      messages: [...messages],
      lastUpdated: new Date(),
      modelName
    };

    if (existingIndex >= 0) {
      conversations[existingIndex] = conversation;
    } else {
      conversations = [conversation, ...conversations];
    }

    saveConversations();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  }

  function clearConversation() {
    if (streamCancel) {
      streamCancel();
      streamCancel = null;
    }
    sending = false;
    messages = [];
    conversationId = undefined;
    input = '';
  }

  function loadConversation(conversation: Conversation) {
    if (streamCancel) {
      streamCancel();
      streamCancel = null;
    }
    sending = false;
    messages = conversation.messages;
    conversationId = conversation.id;
    modelName = conversation.modelName;
    showHistory = false;
  }

  function deleteConversation(id: string, event: Event) {
    event.stopPropagation();
    conversations = conversations.filter(c => c.id !== id);
    saveConversations();

    if (conversationId === id) {
      clearConversation();
    }

  }

  async function deleteAllConversations() {
    if (await dlg.ask(translate('chat.delete_all_confirm'))) {
      conversations = [];
      saveConversations();
      clearConversation();
      showHistory = false;
    }
  }

  function toggleHistory() {
    showHistory = !showHistory;
  }

  const LOCALE_MAP: Record<string, string> = { en: 'en-US', fr: 'fr-FR', de: 'de-DE', es: 'es-ES' };

  function formatTime(date: Date) {
    return date.toLocaleTimeString(LOCALE_MAP[$languageStore] || 'en-US', {
      hour: '2-digit',
      minute: '2-digit',
    });
  }

  function formatDate(date: Date) {
    const now = new Date();
    const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const messageDate = new Date(date.getFullYear(), date.getMonth(), date.getDate());

    if (messageDate.getTime() === today.getTime()) {
      return translate('chat.today');
    }

    const yesterday = new Date(today);
    yesterday.setDate(yesterday.getDate() - 1);
    if (messageDate.getTime() === yesterday.getTime()) {
      return translate('chat.yesterday');
    }

    return date.toLocaleDateString(LOCALE_MAP[$languageStore] || 'en-US', {
      day: 'numeric',
      month: 'short'
    });
  }

  $: suggestions = [
    $t('chat.suggestion_1'),
    $t('chat.suggestion_2'),
    $t('chat.suggestion_3'),
    $t('chat.suggestion_4'),
    $t('chat.suggestion_5'),
    $t('chat.suggestion_6'),
  ];

  $: quickReplies = [
    $t('chat.quick_1'),
    $t('chat.quick_2'),
    $t('chat.quick_3'),
  ];

  function useSuggestion(suggestion: string) {
    input = suggestion;
  }

  /** Day label for the conversation divider, from the first message. */
  $: dayLabel = messages.length > 0 ? formatDate(messages[0].timestamp) : '';

</script>
<PageLayout
  title={$t('chat.title')}
  data-testid="chat-page"
>
  {#snippet icon()}
    <IconBrain stroke={1.7} />
  {/snippet}
  <!-- Connection status: a WindowToolbar row under the header, never inside
       it (the header bar is icon + title + window controls only). -->
  {#snippet toolbarLeft()}
    <span class={cn('inline-flex items-center gap-1.5 h-8 px-3 rounded-full text-xs font-semibold', ollamaStatus === 'online' ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : ollamaStatus === 'checking' ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]' : 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]')}>
      <span class={cn('w-1.5 h-1.5 rounded-full', ollamaStatus === 'online' ? 'bg-[var(--hv-success)]' : ollamaStatus === 'checking' ? 'bg-[var(--hv-warning)]' : 'bg-[var(--hv-error)]')}></span>
      {ollamaStatus === 'checking' ? $t('chat.checking') : ollamaStatus === 'online' ? $t('chat.online') : ollamaStatus === 'nomodel' ? $t('chat.no_model') : $t('chat.offline')}
    </span>
  {/snippet}
  {#snippet toolbarRight()}
    {#if isAdmin}
      <Button variant={showMcp ? 'primary' : 'secondary'} onclick={() => (showMcp = !showMcp)}><IconCpu size={17} stroke={1.9} /> {$t('mcp.title')}</Button>
    {/if}
  {/snippet}

  <div class="flex flex-1 min-h-0">
    {#if showHistory}
      <aside data-testid="chat-history" class="flex flex-col flex-none w-[260px] border-r border-[var(--hv-shell-border)] bg-[var(--hv-shell-fill)]">
        <div class="flex items-center justify-between px-4 pt-4 pb-2">
          <span class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('chat.history_title')}</span>
          <span class="text-xs text-[var(--hv-shell-text-2)]">{$t('chat.saved_count', { count: String(conversations.length) })}</span>
        </div>
        <div class="px-4 pb-2 flex flex-col gap-2">
          <Button variant="secondary" fullWidth onclick={() => { clearConversation(); showHistory = false; }}><IconPlus size={20} stroke={2} /> {$t('chat.new_conversation')}</Button>
          {#if conversations.length > 0}
            <Button variant="danger" fullWidth onclick={deleteAllConversations}><IconTrash size={16} stroke={2} /> {$t('chat.delete_all')}</Button>
          {/if}
        </div>
        <div class="flex-1 min-h-0 overflow-y-auto px-2 pb-3 flex flex-col gap-1">
          {#if conversations.length === 0}
            <div class="flex flex-col items-center gap-2 py-8 text-center text-[var(--hv-shell-text-2)]">
              <IconHistory size={32} stroke={1.5} />
              <p class="m-0 text-xs">{$t('chat.no_conversations')}</p>
            </div>
          {:else}
            {#each conversations as conv}
              <div class={cn('group flex items-center gap-1 px-2 py-2 rounded-lg cursor-pointer hover:bg-[var(--hv-shell-border-soft)]', conv.id === conversationId && 'bg-[var(--hv-primary-light)]')}>
                <button class="flex-1 min-w-0 text-left bg-transparent border-none cursor-pointer" onclick={() => loadConversation(conv)}>
                  <span class="block text-c-base font-semibold text-[var(--hv-shell-text-strong)] truncate">{conv.title}</span>
                  <span class="block text-c-xs text-[var(--hv-shell-text-2)]"><span>{$t('chat.messages_count', { count: String(conv.messages.length) })}</span> · <span>{formatDate(conv.lastUpdated)}</span></span>
                </button>
                <button class="flex-none grid place-items-center w-8 h-8 rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 hover:text-[var(--hv-error)] hv-ic" onclick={(e) => deleteConversation(conv.id, e)} title={$t('chat.delete_conversation_title')} aria-label={$t('chat.delete_conversation_title')}><IconTrash size={16} stroke={2} /></button>
              </div>
            {/each}
          {/if}
        </div>
      </aside>
    {/if}

    <div class="flex flex-col flex-1 min-w-0 min-h-0">
      <div class="flex-1 min-h-0 overflow-y-auto" bind:this={messagesContainer}>
        {#if messages.length === 0}
          <EmptyState data-testid="chat-empty" title={$t('chat.empty_title')}>
            {#snippet iconSnippet()}<IconBrain size={32} stroke={1.5} />{/snippet}
            {#snippet action()}
              <div class="flex flex-col items-center gap-3">
            {#if ollamaStatus === 'offline' || ollamaStatus === 'nomodel'}
              <p class="m-0 text-sm text-[var(--hv-warning)]">{ollamaStatus === 'nomodel' ? $t('chat.nomodel_hint') : $t('chat.offline_hint')}</p>
            {:else}
              <p class="m-0 text-sm text-[var(--hv-shell-text)]">{$t('chat.empty_desc', { model: modelName })}</p>
            {/if}
            <div class="grid gap-2 w-full max-w-[420px] mt-2">
              {#each suggestions as suggestion}
                <button data-testid="chat-suggestion" class="flex items-start gap-3 px-4 py-3 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-left cursor-pointer hover:border-[var(--hv-primary)]/50 transition-colors" onclick={() => useSuggestion(suggestion)}>
                  <span class="text-[var(--hv-primary)] flex-none"><IconSparkles size={24} stroke={1.8} /></span>
                  <span data-testid="chat-suggestion-text" class="text-sm text-[var(--hv-shell-text-strong)]">{suggestion}</span>
                </button>
              {/each}
            </div>
              </div>
            {/snippet}
          </EmptyState>
        {:else}
          <div class="px-4 py-4 flex flex-col gap-4 max-w-[820px] mx-auto">
            <div class="flex items-center gap-2 text-c-xs uppercase tracking-wide text-[var(--hv-shell-text-3)]"><span class="h-px flex-1 bg-[var(--hv-shell-border-soft)]"></span><span>{dayLabel}</span><span class="h-px flex-1 bg-[var(--hv-shell-border-soft)]"></span></div>
            {#each messages as msg, i}
              {#if msg.role === 'assistant'}
                <div data-testid="chat-row" class="hv-chat-row-bot flex items-start gap-2.5">
                  <span class="grid place-items-center w-9 h-9 rounded-full flex-none bg-[var(--hv-app-chat-grad)] text-white"><IconBrain size={26} stroke={1.7} /></span>
                  <div data-testid="chat-bubble" class="hv-chat-bubble-bot max-w-[85%] px-4 py-3 rounded-2xl rounded-tl-md bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
                    {#if msg.content === '' && sending && i === messages.length - 1}
                      <div class="hv-chat-typing flex gap-1"><span></span><span></span><span></span></div>
                    {:else}
                      <div data-testid="chat-bubble-text" class="text-sm text-[var(--hv-shell-text-strong)] leading-relaxed">{@html renderMessageContent(msg.content)}</div>
                      <div class="flex items-center gap-2 mt-1 text-c-xs text-[var(--hv-shell-text-2)]">
                        <span>{formatTime(msg.timestamp)}</span>
                        <span>{modelName}</span>
                        {#if msg.content}
                          <button data-testid="chat-action" class="border-none bg-transparent cursor-pointer text-[var(--hv-shell-text-2)] hover:text-[var(--hv-shell-text-strong)]" onclick={() => copyMessage(i, msg.content)} title={$t('chat.copy_message')} aria-label={$t('chat.copy_message')}>{#if copiedIndex === i}<IconCheck size={14} stroke={2} />{:else}<IconCopy size={14} stroke={2} />{/if}</button>
                          {#if i === messages.length - 1 && !sending}
                            <button data-testid="chat-action" class="border-none bg-transparent cursor-pointer text-[var(--hv-shell-text-2)] hover:text-[var(--hv-shell-text-strong)]" onclick={regenerateLast} title={$t('chat.regenerate')} aria-label={$t('chat.regenerate')}><IconRefresh size={14} stroke={2} /></button>
                          {/if}
                        {/if}
                      </div>
                    {/if}
                  </div>
                </div>
              {:else}
                <div data-testid="chat-row" class="hv-chat-row-user flex items-start justify-end gap-2.5">
                  <div data-testid="chat-bubble" class="hv-chat-bubble-user max-w-[85%] px-4 py-3 rounded-2xl rounded-tr-md bg-[var(--hv-primary)] text-white">
                    <div data-testid="chat-bubble-text" class="text-sm leading-relaxed">{@html renderMessageContent(msg.content)}</div>
                    <div class="flex items-center gap-2 mt-1 text-c-xs opacity-80">
                      <span>{formatTime(msg.timestamp)}</span>
                      <span>{$t('chat.you')}</span>
                    </div>
                  </div>
                </div>
              {/if}
            {/each}
          </div>
        {/if}
      </div>

      <div class="flex-none border-t border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] px-4 py-3">
        {#if messages.length > 0}
          <div class="flex flex-wrap gap-1.5 mb-2">
            {#each quickReplies as q}
              <button class="px-3 py-1.5 rounded-full border border-[var(--hv-shell-border)] bg-[var(--hv-shell-fill)] text-xs text-[var(--hv-shell-text-2)] cursor-pointer hover:border-[var(--hv-primary)]/50 hover:text-[var(--hv-primary)]" onclick={() => useSuggestion(q)}>{q}</button>
            {/each}
          </div>
        {/if}
        <div class="flex items-center gap-2">
          <button data-testid="chat-history-btn" class={cn('grid place-items-center w-12 h-12 flex-none rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] cursor-pointer', showHistory && 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')} class:active={showHistory} onclick={toggleHistory} title={$t('chat.history_title')} aria-label={$t('chat.history_title')}><IconHistory size={26} stroke={1.9} /></button>
          <div class="flex-1 min-w-0">
            <textarea data-testid="chat-input" class="w-full resize-none border border-[var(--hv-shell-border-strong)] rounded-md bg-[var(--hv-shell-border-soft)] px-4 py-3 text-sm text-[var(--hv-shell-text-strong)] outline-none focus:border-[var(--hv-primary)]" bind:value={input} onkeydown={handleKeydown} placeholder={$t('chat.input_placeholder')} rows="1" disabled={sending}></textarea>
          </div>
          {#if sending}
            <button data-testid="chat-send" class="grid place-items-center w-12 h-12 flex-none rounded-lg border-none bg-[var(--hv-error)] text-white cursor-pointer hover:brightness-110" onclick={stopGenerating} title={$t('chat.stop_generating')} aria-label={$t('chat.stop_generating')}><IconSquare size={22} stroke={2} fill="currentColor" /></button>
          {:else}
            <button data-testid="chat-send" class="grid place-items-center w-12 h-12 flex-none rounded-lg border-none bg-[var(--hv-app-chat-grad)] text-white cursor-pointer disabled:opacity-40 disabled:cursor-default" onclick={sendMessage} disabled={!input.trim()} title={$t('chat.send')} aria-label={$t('chat.send')}><IconSend size={26} stroke={1.9} /></button>
          {/if}
        </div>
        <div class="mt-1.5 text-center text-c-xs text-[var(--hv-shell-text-3)]">{$t('chat.local_note')}</div>
      </div>
    </div>
    {#if showMcp && isAdmin}
      <McpPanel onclose={() => (showMcp = false)} />
    {/if}
  </div>
  <ConfirmDialog
    {dlg}
    onconfirm={() => dlg.settle(true)}
    oncancel={() => dlg.settle(false)}
  />
</PageLayout>