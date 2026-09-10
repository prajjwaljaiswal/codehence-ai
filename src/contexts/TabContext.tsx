import React, { createContext, useState, useContext, useCallback, useEffect, useRef } from 'react';
import { TabPersistenceService } from '@/services/tabPersistence';
import { SessionPersistenceService } from '@/services/sessionPersistence';
import { api } from '@/lib/api';

export interface Tab {
  id: string;
  type: 'chat' | 'agent' | 'agents' | 'projects' | 'usage' | 'tasks' | 'mcp' | 'settings' | 'claude-md' | 'claude-file' | 'agent-execution' | 'create-agent' | 'import-agent';
  title: string;
  sessionId?: string;  // for chat tabs
  sessionData?: any; // for chat tabs - stores full session object
  agentRunId?: string; // for agent tabs
  agentData?: any; // for agent-execution tabs
  claudeFileId?: string; // for claude-file tabs
  initialProjectPath?: string; // for chat tabs
  projectPath?: string; // for agent-execution and tasks tabs
  status: 'active' | 'idle' | 'running' | 'complete' | 'error';
  /** A response finished while this tab was in the background — drives the
   *  tab's unread dot and the app icon's badge count until it's viewed */
  hasUnreadResponse?: boolean;
  hasUnsavedChanges: boolean;
  order: number;
  icon?: string;
  createdAt: Date;
  updatedAt: Date;
}

interface TabContextType {
  tabs: Tab[];
  activeTabId: string | null;
  addTab: (tab: Omit<Tab, 'id' | 'order' | 'createdAt' | 'updatedAt'>) => string;
  removeTab: (id: string) => void;
  updateTab: (id: string, updates: Partial<Tab>) => void;
  setActiveTab: (id: string) => void;
  reorderTabs: (startIndex: number, endIndex: number) => void;
  getTabById: (id: string) => Tab | undefined;
  closeAllTabs: () => void;
  getTabsByType: (type: 'chat' | 'agent') => Tab[];
  /** Flag a tab as having a response the user hasn't seen, and notify them.
   *  No-ops when the tab is already visible to the user. */
  notifyTabResponseReady: (id: string) => void;
  /** Number of tabs with responses the user hasn't looked at yet */
  unreadResponseCount: number;
}

const TabContext = createContext<TabContextType | undefined>(undefined);

// const STORAGE_KEY = 'opcode_tabs'; // No longer needed - persistence disabled
const MAX_TABS = 20;

export const TabProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);
  const isInitialized = useRef(false);
  const saveTimeoutRef = useRef<NodeJS.Timeout>();

  // Mirrors of the state above, for callbacks that must read the current
  // values without being re-created (and re-subscribed) on every change.
  const tabsRef = useRef<Tab[]>(tabs);
  tabsRef.current = tabs;
  const activeTabIdRef = useRef<string | null>(activeTabId);
  activeTabIdRef.current = activeTabId;
  const isWindowFocusedRef = useRef(
    typeof document === 'undefined' ? true : document.hasFocus()
  );

  const unreadResponseCount = tabs.filter(tab => tab.hasUnreadResponse).length;

  // Keep the app icon's badge in step with the unread count
  useEffect(() => {
    api.setAppBadgeCount(unreadResponseCount || null).catch(err =>
      console.error('Failed to update app badge count:', err)
    );
  }, [unreadResponseCount]);

  // Load tabs from storage on mount
  useEffect(() => {
    const loadTabs = async () => {
    if (isInitialized.current) return;
    isInitialized.current = true;

    // Migrate from old format if needed
    TabPersistenceService.migrateFromOldFormat();

    // Try to load saved tabs
    const { tabs: savedTabs, activeTabId: savedActiveTabId } = TabPersistenceService.loadTabs();
    
    if (savedTabs.length > 0) {
      // For chat tabs, restore session data
      const restoredTabs = await Promise.all(savedTabs.map(async (tab) => {
        if (tab.type === 'chat' && tab.sessionId) {
          // Check if session can be restored
          const sessionData = SessionPersistenceService.loadSession(tab.sessionId);
          if (sessionData) {
            // Create a Session object for the tab
            const session = SessionPersistenceService.createSessionFromRestoreData(sessionData);
            return {
              ...tab,
              sessionData: session,
              initialProjectPath: sessionData.projectPath
            };
          }
        }
        return tab;
      }));
      
      setTabs(restoredTabs);
      setActiveTabId(savedActiveTabId);
    } else {
      // Create default projects tab if no saved tabs
      const defaultTab: Tab = {
        id: generateTabId(),
        type: 'projects',
        title: 'Projects',
        status: 'idle',
        hasUnsavedChanges: false,
        order: 0,
        createdAt: new Date(),
        updatedAt: new Date()
      };
      setTabs([defaultTab]);
      setActiveTabId(defaultTab.id);
    }
    };
    
    loadTabs();
  }, []);

  // Save tabs to localStorage with debounce
  useEffect(() => {
    // Don't save if not initialized
    if (!isInitialized.current) return;
    
    // Clear existing timeout
    if (saveTimeoutRef.current) {
      clearTimeout(saveTimeoutRef.current);
    }

    // Debounce saving to avoid excessive writes
    saveTimeoutRef.current = setTimeout(() => {
      TabPersistenceService.saveTabs(tabs, activeTabId);
    }, 500); // Wait 500ms after last change before saving

    return () => {
      if (saveTimeoutRef.current) {
        clearTimeout(saveTimeoutRef.current);
      }
    };
  }, [tabs, activeTabId]);

  // Save tabs immediately when window is about to close
  useEffect(() => {
    const handleBeforeUnload = () => {
      if (isInitialized.current && tabs.length > 0) {
        TabPersistenceService.saveTabs(tabs, activeTabId);
      }
    };

    window.addEventListener('beforeunload', handleBeforeUnload);
    
    return () => {
      window.removeEventListener('beforeunload', handleBeforeUnload);
      // Save one final time when component unmounts
      if (isInitialized.current && tabs.length > 0) {
        TabPersistenceService.saveTabs(tabs, activeTabId);
      }
    };
  }, [tabs, activeTabId]);

  const generateTabId = () => {
    return `tab-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
  };

  const addTab = useCallback((tabData: Omit<Tab, 'id' | 'order' | 'createdAt' | 'updatedAt'>): string => {
    if (tabs.length >= MAX_TABS) {
      throw new Error(`Maximum number of tabs (${MAX_TABS}) reached`);
    }

    const newTab: Tab = {
      ...tabData,
      id: generateTabId(),
      order: tabs.length,
      createdAt: new Date(),
      updatedAt: new Date()
    };

    setTabs(prevTabs => [...prevTabs, newTab]);
    setActiveTabId(newTab.id);
    return newTab.id;
  }, [tabs.length]);

  const removeTab = useCallback((id: string) => {
    setTabs(prevTabs => {
      const filteredTabs = prevTabs.filter(tab => tab.id !== id);
      
      // Reorder remaining tabs
      const reorderedTabs = filteredTabs.map((tab, index) => ({
        ...tab,
        order: index
      }));

      // Update active tab if necessary
      if (activeTabId === id && reorderedTabs.length > 0) {
        const removedTabIndex = prevTabs.findIndex(tab => tab.id === id);
        const newActiveIndex = Math.min(removedTabIndex, reorderedTabs.length - 1);
        setActiveTabId(reorderedTabs[newActiveIndex].id);
      } else if (reorderedTabs.length === 0) {
        setActiveTabId(null);
      }

      return reorderedTabs;
    });
  }, [activeTabId]);

  const updateTab = useCallback((id: string, updates: Partial<Tab>) => {
    setTabs(prevTabs =>
      prevTabs.map(tab =>
        tab.id === id
          ? { ...tab, ...updates, updatedAt: new Date() }
          : tab
      )
    );
  }, []);

  const clearUnread = useCallback((id: string) => {
    setTabs(prevTabs => {
      if (!prevTabs.some(tab => tab.id === id && tab.hasUnreadResponse)) {
        return prevTabs; // Nothing to clear — don't churn state
      }
      return prevTabs.map(tab =>
        tab.id === id ? { ...tab, hasUnreadResponse: false } : tab
      );
    });
  }, []);

  /**
   * Called when a session finishes responding. If the user can already see
   * that tab (it's active and the window is focused) there's nothing to
   * announce; otherwise flag it unread and post a desktop notification.
   */
  const notifyTabResponseReady = useCallback((id: string) => {
    const tab = tabsRef.current.find(t => t.id === id);
    if (!tab) return;

    const userIsLookingAtIt = isWindowFocusedRef.current && activeTabIdRef.current === id;
    if (userIsLookingAtIt) return;

    setTabs(prevTabs =>
      prevTabs.map(t => (t.id === id ? { ...t, hasUnreadResponse: true } : t))
    );

    api
      .showDesktopNotification('Response ready', `Claude finished responding in ${tab.title}`)
      .catch(err => console.error('Failed to post response notification:', err));
  }, []);

  const setActiveTab = useCallback((id: string) => {
    if (tabs.find(tab => tab.id === id)) {
      setActiveTabId(id);
      // Looking at a tab counts as reading it
      clearUnread(id);
    }
  }, [tabs, clearUnread]);

  const reorderTabs = useCallback((startIndex: number, endIndex: number) => {
    setTabs(prevTabs => {
      const newTabs = [...prevTabs];
      const [removed] = newTabs.splice(startIndex, 1);
      newTabs.splice(endIndex, 0, removed);
      
      // Update order property
      return newTabs.map((tab, index) => ({
        ...tab,
        order: index
      }));
    });
  }, []);

  const getTabById = useCallback((id: string): Tab | undefined => {
    return tabs.find(tab => tab.id === id);
  }, [tabs]);

  const closeAllTabs = useCallback(() => {
    setTabs([]);
    setActiveTabId(null);
    TabPersistenceService.clearTabs();
  }, []);

  const getTabsByType = useCallback((type: 'chat' | 'agent'): Tab[] => {
    return tabs.filter(tab => tab.type === type);
  }, [tabs]);

  // Track window focus, so a response that lands while the app is in the
  // background still counts as unread — and so returning to the app marks
  // whatever tab is on screen as read.
  useEffect(() => {
    const handleFocus = () => {
      isWindowFocusedRef.current = true;
      if (activeTabIdRef.current) {
        clearUnread(activeTabIdRef.current);
      }
    };
    const handleBlur = () => {
      isWindowFocusedRef.current = false;
    };

    window.addEventListener('focus', handleFocus);
    window.addEventListener('blur', handleBlur);
    return () => {
      window.removeEventListener('focus', handleFocus);
      window.removeEventListener('blur', handleBlur);
    };
  }, [clearUnread]);

  const value: TabContextType = {
    tabs,
    activeTabId,
    addTab,
    removeTab,
    updateTab,
    setActiveTab,
    reorderTabs,
    getTabById,
    closeAllTabs,
    getTabsByType,
    notifyTabResponseReady,
    unreadResponseCount
  };

  return (
    <TabContext.Provider value={value}>
      {children}
    </TabContext.Provider>
  );
};

export const useTabContext = () => {
  const context = useContext(TabContext);
  if (!context) {
    throw new Error('useTabContext must be used within a TabProvider');
  }
  return context;
};
