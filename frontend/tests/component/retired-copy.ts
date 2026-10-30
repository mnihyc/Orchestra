/** Exact retired UI prose remains a negative regression fixture, never a product catalogue entry. */
const RETIRED = {
  'approvals.lead': ['What agents and colleagues ask leave for: what is yours to decide, what you asked for, and what was decided.', '智能体和同事请求许可的事项：待你决定的、你发起的，以及已经决定的。'],
  'approvals.everything': ['Everything I could decide', '显示我有权决定的全部'],
  'auth.second.lead.totp': ['Enter the code your authenticator app shows for this account.', '请输入身份验证器应用中为这个账号显示的验证码。'],
  'day100.views.scopeOwn': ['Topics directly in {{name}}', '直接位于{{name}}中的话题'],
  'shell.keyChoice.single.hint': ['Keys without Ctrl, Alt or Command, such as G I, ? and /, act when you are not typing in a field. Off, only keys with one of those act.', '不带 Ctrl、Alt 或 Command 的按键（如 G I、? 和 /）会在你未在输入框中输入时生效。关闭后，只有带这些修饰键的快捷键生效。'],
  'settings.interface.viewers.lead': ['Where you chose a viewer for a kind of file, it is listed here.', '你为某类文件选定的查看器会列在这里。'],
  'everything.lead': ['Every place and thing you can see, with the details you choose.', '查看你可见的所有空间和内容，自行选择要显示的详情。'],
  'everything.keyboard': ['Use Up and Down to move, Right and Left to expand or collapse, and Enter to open.', '使用上下方向键移动，左右方向键展开或收起，按回车键打开。'],
  'shell.unreachableLead': ['The server did not answer', '服务器没有响应'],
  'access.noAccess.lead': ['What this takes is listed here, with whom to ask for it.', '访问它需要的权限，以及可以请求的人，列在下面。'],
  'settings.general.dangerLead': ['Each says what it does before it does it, and until when it can be taken back.', '每一项在执行前都会说明它做什么，以及在什么时候之前可以撤销。'],
  'settings.org.lead': ['The same sections at every level: a department has its own members, groups, tags and policies.', '每一级都有同样的部分：部门有自己的成员、群组、标签和策略。'],
  'timeline.step.raw': ['Raw', '原始调用'],
} as const;

export function retiredCopy(locale: string, key: keyof typeof RETIRED, values: Readonly<Record<string, string>> = {}): string {
  let words: string = RETIRED[key][locale.startsWith('zh') ? 1 : 0];
  for (const [name, value] of Object.entries(values)) words = words.replaceAll(`{{${name}}}`, value);
  return words;
}
