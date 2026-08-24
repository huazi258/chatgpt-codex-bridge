import { useState } from 'react';
import { phaseLabel, type RelayModule } from '../relay-ui';

interface SessionSidebarProps {
  sessions: RelayModule[];
  selectedId: string | null;
  creating: boolean;
  collapsed: boolean;
  busy: boolean;
  onCollapsedChange: (collapsed: boolean) => void;
  onCreate: () => void;
  onSelect: (id: string) => void;
  onOpenDirectory: (session: RelayModule) => void;
  onDelete: (session: RelayModule) => void;
}

export function SessionSidebar(props: SessionSidebarProps) {
  const [menuId, setMenuId] = useState<string | null>(null);
  const { collapsed } = props;
  return <aside className={`session-sidebar ${collapsed ? 'collapsed' : ''}`}>
    <div className="sidebar-heading">
      <button className="icon-button" type="button" title={collapsed ? '展开会话列表' : '收起会话列表'} aria-label={collapsed ? '展开会话列表' : '收起会话列表'} onClick={() => props.onCollapsedChange(!collapsed)}>☰</button>
      {!collapsed ? <div><p className="eyebrow">会话 Sessions</p><strong>传话工作台</strong></div> : null}
    </div>
    <button className="new-session" type="button" disabled={props.busy} onClick={props.onCreate} title="新建会话">＋ {!collapsed ? '新建会话' : ''}</button>
    <nav aria-label="会话列表" className="session-list">
      {props.sessions.length === 0 && !collapsed ? <p className="empty">还没有会话。</p> : null}
      {props.sessions.map((session) => <div className={`session-entry ${!props.creating && props.selectedId === session.id ? 'selected' : ''}`} key={session.id}>
        <button className="session-select" type="button" disabled={props.busy} onClick={() => props.onSelect(session.id)} title={collapsed ? session.name : undefined}>
          <span className={`phase-dot phase-${session.phase.toLowerCase()}`} />
          {!collapsed ? <span className="session-name"><strong>{session.name}</strong><small>{phaseLabel(session.phase)}</small></span> : null}
        </button>
        {!collapsed ? <button className="more-button" type="button" aria-label={`打开“${session.name}”菜单`} onClick={() => setMenuId(menuId === session.id ? null : session.id)}>•••</button> : null}
        {!collapsed && menuId === session.id ? <div className="session-menu">
          <button type="button" onClick={() => { setMenuId(null); props.onOpenDirectory(session); }}>打开工作目录</button>
          <button className="danger-text" type="button" onClick={() => { setMenuId(null); props.onDelete(session); }}>删除会话</button>
        </div> : null}
      </div>)}
    </nav>
  </aside>;
}
