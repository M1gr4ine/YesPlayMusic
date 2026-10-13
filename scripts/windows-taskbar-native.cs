// Taskbar helper for the Windows taskbar strip, built by src-tauri/build.rs.
// It reads Explorer's taskbars and parents the overlay into one; it never
// injects code into Explorer. Requests and replies are JSON lines on stdio.
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
using System.Web.Script.Serialization;
using Accessibility;

public static class TaskbarNative {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int left,top,right,bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int x,y; }
  [StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int size; public RECT monitor,work; public uint flags; }
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] static extern IntPtr MonitorFromWindow(IntPtr h,uint flags);
  [DllImport("user32.dll")] static extern bool GetMonitorInfo(IntPtr h,ref MONITORINFO info);
  delegate bool EnumProc(IntPtr hwnd,IntPtr lparam);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc fn,IntPtr arg);
  [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr h,EnumProc fn,IntPtr arg);
  [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h,StringBuilder b,int n);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h,out RECT r);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h,int command);
  [DllImport("user32.dll")] static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll",SetLastError=true)] static extern IntPtr SetParent(IntPtr h,IntPtr parent);
  [DllImport("user32.dll")] static extern IntPtr GetWindowLongPtr(IntPtr h,int index);
  [DllImport("user32.dll",SetLastError=true)] static extern IntPtr SetWindowLongPtr(IntPtr h,int index,IntPtr value);
  [DllImport("kernel32.dll",SetLastError=true)] static extern void SetLastError(uint error);
  [DllImport("user32.dll",SetLastError=true)] static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int height,uint flags);
  [DllImport("user32.dll",SetLastError=true)] static extern bool ScreenToClient(IntPtr h,ref POINT p);
  [DllImport("user32.dll",SetLastError=true)] static extern bool GetClientRect(IntPtr h,out RECT r);
  [DllImport("user32.dll",SetLastError=true)] static extern bool ClientToScreen(IntPtr h,ref POINT p);
  [DllImport("user32.dll")] static extern bool RedrawWindow(IntPtr h,ref RECT update,IntPtr region,uint flags);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
  [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
  [DllImport("user32.dll")] static extern IntPtr GetWindowDpiAwarenessContext(IntPtr h);
  [DllImport("user32.dll")] static extern int GetAwarenessFromDpiAwarenessContext(IntPtr c);
  [DllImport("oleacc.dll")] static extern int AccessibleObjectFromWindow(IntPtr h,uint id,ref Guid iid,[MarshalAs(UnmanagedType.Interface)] out IAccessible a);
  static readonly JavaScriptSerializer Json=new JavaScriptSerializer();
  static IntPtr window,parent,attachedTo,oldParent,oldStyle,oldExStyle;
  static RECT oldRect;
  static uint owner;
  static int frameExtraWidth,frameExtraHeight;
  static int clientLeftOffset,clientTopOffset;
  static bool probeComplete;
  static string Class(IntPtr h) {var b=new StringBuilder(256);GetClassName(h,b,256);return b.ToString();}
  static Dictionary<string,object> Rect(int x,int y,int w,int h) { return new Dictionary<string,object>{{"x",x},{"y",y},{"width",w},{"height",h}}; }
  static Dictionary<string,object> Rect(RECT r) {return Rect(r.left,r.top,r.right-r.left,r.bottom-r.top);}
  static bool OwnWindow() {uint pid;GetWindowThreadProcessId(window,out pid);return IsWindow(window)&&pid==owner;}
  static void SetLongChecked(int index,IntPtr value) {
    SetLastError(0);IntPtr previous=SetWindowLongPtr(window,index,value);int error=Marshal.GetLastWin32Error();
    if(previous==IntPtr.Zero&&error!=0)throw new System.ComponentModel.Win32Exception(error);
  }
  static void SetParentChecked(IntPtr expected) {
    SetLastError(0);IntPtr previous=SetParent(window,expected);int error=Marshal.GetLastWin32Error();
    if(previous==IntPtr.Zero&&error!=0)throw new System.ComponentModel.Win32Exception(error);
    // While WS_CHILD is still set, SetParent(NULL) can report the desktop as
    // the effective parent. Verify only after the top-level styles are restored.
  }
  static bool EnsureTaskbarChild(IntPtr host) {
    const long WS_POPUP=0x80000000L,WS_CHILD=0x40000000L;
    const long nonClientStyle=0x00C40000L,nonClientExStyle=0x00020301L;
    long style=GetWindowLongPtr(window,-16).ToInt64(),exStyle=GetWindowLongPtr(window,-20).ToInt64();
    long dockStyle=(style&~(WS_POPUP|nonClientStyle))|WS_CHILD,dockExStyle=exStyle&~nonClientExStyle;
    bool alreadyChild=GetParent(window)==host;
    bool changed=!alreadyChild||style!=dockStyle||exStyle!=dockExStyle;
    if(changed){
      // Visibility changes can restore Tao's cached top-level frame. Preserve
      // live state bits while removing the taskbar child's non-client borders.
      if(style!=dockStyle)SetLongChecked(-16,new IntPtr(dockStyle));
      if(exStyle!=dockExStyle)SetLongChecked(-20,new IntPtr(dockExStyle));
      if(GetParent(window)!=host)SetParentChecked(host);
      if(!SetWindowPos(window,IntPtr.Zero,0,0,0,0,0x37))
        throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
    }
    long verifiedStyle=GetWindowLongPtr(window,-16).ToInt64(),verifiedExStyle=GetWindowLongPtr(window,-20).ToInt64();
    if(GetParent(window)!=host||(verifiedStyle&WS_CHILD)==0||
      (verifiedStyle&(WS_POPUP|nonClientStyle))!=0||(verifiedExStyle&nonClientExStyle)!=0)
      throw new Exception("Windows did not keep the borderless player attached to the taskbar");
    MeasureFrameExtent();
    // A restyled child may already have painted a caption into Explorer's
    // surface, which neither process repaints. Let the taskbar redraw it.
    if(changed&&alreadyChild)RepaintBeneath(host);
    return changed;
  }
  static void RepaintBeneath(IntPtr host) {
    RECT r;if(!GetWindowRect(window,out r))return;
    POINT a=new POINT{x=r.left,y=r.top},b=new POINT{x=r.right,y=r.bottom};
    if(!ScreenToClient(host,ref a)||!ScreenToClient(host,ref b))return;
    RECT local=new RECT{left=a.x,top=a.y,right=b.x,bottom=b.y};
    bool visible=IsWindowVisible(window);
    if(visible)ShowWindow(window,0);
    // RDW_INVALIDATE|RDW_ERASE|RDW_ALLCHILDREN|RDW_UPDATENOW
    RedrawWindow(host,ref local,IntPtr.Zero,0x0001|0x0004|0x0080|0x0100);
    if(visible)ShowWindow(window,4);
  }
  static void MeasureFrameExtent() {
    RECT outer;
    if(!GetWindowRect(window,out outer))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
    RECT client=ClientRectOnScreen(window);
    frameExtraWidth=checked(outer.right-outer.left-(client.right-client.left));
    frameExtraHeight=checked(outer.bottom-outer.top-(client.bottom-client.top));
    clientLeftOffset=checked(client.left-outer.left);clientTopOffset=checked(client.top-outer.top);
    if(frameExtraWidth<0||frameExtraHeight<0||frameExtraWidth>96||frameExtraHeight>96||
      clientLeftOffset<0||clientTopOffset<0||clientLeftOffset>frameExtraWidth||clientTopOffset>frameExtraHeight)
      throw new Exception("Player non-client geometry is outside the supported range");
  }
  static void RepositionAfterFrameChange(IntPtr host) {
    var bar=FindBar(host.ToInt64().ToString());
    Place(Placement(bar),bar);
  }
  static bool? FullscreenOnMonitor(IntPtr bar,out RECT monitorBounds) {
    monitorBounds=new RECT();
    IntPtr monitor=MonitorFromWindow(bar,2);
    if(monitor==IntPtr.Zero)return null;
    MONITORINFO info=new MONITORINFO();info.size=Marshal.SizeOf(typeof(MONITORINFO));
    if(!GetMonitorInfo(monitor,ref info))return null;
    // Every taskbar observation needs valid monitor bounds, even when the
    // foreground is Explorer or belongs to a different monitor.
    monitorBounds=info.monitor;
    IntPtr fg=GetForegroundWindow();
    if(fg==IntPtr.Zero)return null;
    if(fg==bar)return false;
    if(!IsWindow(fg))return null;
    if(!IsWindowVisible(fg))return false;
    string cls=Class(fg);
    if(String.IsNullOrEmpty(cls))return null;
    if(cls=="Progman"||cls=="WorkerW"||cls=="Shell_TrayWnd"||cls=="Shell_SecondaryTrayWnd")return false;
    IntPtr foregroundMonitor=MonitorFromWindow(fg,2);
    if(foregroundMonitor==IntPtr.Zero)return null;
    if(foregroundMonitor!=monitor)return false;
    RECT rect;
    if(!GetWindowRect(fg,out rect))return null;
    return rect.left<=info.monitor.left&&rect.top<=info.monitor.top&&rect.right>=info.monitor.right&&rect.bottom>=info.monitor.bottom;
  }
  static Dictionary<string,object> Observation(IntPtr bar,RECT r,RECT monitor,bool suspended) {
    return new Dictionary<string,object>{{"handle",bar.ToInt64().ToString()},{"taskbar",Rect(r)},{"monitor",Rect(monitor)},{"free",null},{"dpi",GetDpiForWindow(bar)},{"suspended",suspended}};
  }
  static List<Dictionary<string,object>> Probe() {
    probeComplete=true;
    var result=new List<Dictionary<string,object>>();
    bool enumerated=EnumWindows(delegate(IntPtr bar,IntPtr arg) {
      string cls=Class(bar);
      if(cls!="Shell_TrayWnd"&&cls!="Shell_SecondaryTrayWnd")return true;
      RECT r;if(!GetWindowRect(bar,out r)){probeComplete=false;return true;}
      int width=r.right-r.left,height=r.bottom-r.top;
      bool horizontal=width>=height&&height>=24&&height<=120;
      bool vertical=height>width&&width>=24&&width<=120;
      if(!horizontal&&!vertical)return true;
      // Unknown foreground/monitor reads fail closed, like a confirmed fullscreen window.
      RECT monitor;bool? fullscreen=FullscreenOnMonitor(bar,out monitor); bool suspended=!IsWindowVisible(bar)||fullscreen!=false;
      if(monitor.right<=monitor.left||monitor.bottom<=monitor.top){probeComplete=false;return true;}
      var observation=Observation(bar,r,monitor,suspended); observation["fullscreen"]=fullscreen;
      result.Add(observation);
      // The supported dock is a horizontal 380-DIP strip. Keep vertical taskbar
      // observations for fullscreen safety, but do not invent free space there.
      if(suspended||!horizontal)return true;
      IntPtr tasklist=IntPtr.Zero;
      EnumChildWindows(bar,delegate(IntPtr child,IntPtr unused){if(Class(child)=="MSTaskListWClass")tasklist=child;return true;},IntPtr.Zero);
      if(tasklist==IntPtr.Zero)return true;
      RECT list;if(!GetWindowRect(tasklist,out list)){probeComplete=false;return true;}
      int occupied=list.left;
      IAccessible a=null;
      try {
        Guid iid=new Guid("618736e0-3c3d-11cf-810c-00aa00389b71");
        if(AccessibleObjectFromWindow(tasklist,0xfffffffc,ref iid,out a)!=0||a==null)return true;
        int count=a.accChildCount;
        if(count>512)return true;
        for(int i=1;i<=count;i++){
          int x,y,w,h;
          try {a.accLocation(out x,out y,out w,out h,i);}
          catch {return true;} // Unknown occupancy is not free space.
          if(w>0&&h>0&&y<r.bottom&&y+h>r.top)occupied=Math.Max(occupied,x+w);
        }
      } catch {return true;}
      finally {if(a!=null)Marshal.ReleaseComObject(a);}
      int gap=0;
      int left=Math.Max(list.left,occupied)+gap, right=r.right;
      // Use the full blank band up to the next real shell control (weather/tray).
      // Ignore the lyrics child and the task-list containers themselves.
      EnumChildWindows(bar,delegate(IntPtr child,IntPtr unused){
        if(GetParent(child)!=bar||child==window||!IsWindowVisible(child))return true;
        RECT cr;if(!GetWindowRect(child,out cr)){probeComplete=false;return true;}
        if(cr.right>cr.left&&cr.left>=list.right&&cr.top<r.bottom&&cr.bottom>r.top)right=Math.Min(right,cr.left);
        return true;
      },IntPtr.Zero);
      right-=(int)Math.Ceiling(4*GetDpiForWindow(bar)/96.0);
      observation["free"]=Rect(left,r.top,Math.Max(0,right-left),r.bottom-r.top);
      return true;
    },IntPtr.Zero);
    if(!enumerated)probeComplete=false;
    return result;
  }
  static object ProbeResult() {var bars=Probe();return new {complete=probeComplete,bars=bars};}
  static Dictionary<string,object> FindBar(string handle) {foreach(var bar in Probe())if((string)bar["handle"]==handle){if(!probeComplete)throw new Exception("TASKBAR_PROBE_UNKNOWN");return bar;}throw new Exception("Taskbar is unavailable");}
  static Dictionary<string,object> Placement(Dictionary<string,object> bar) {
    if((bool)bar["suspended"])throw new Exception("TASKBAR_SUSPENDED");
    if(bar["free"]==null)throw new Exception("Taskbar layout temporarily unavailable");
    var free=(Dictionary<string,object>)bar["free"];
    double scale=Convert.ToDouble(bar["dpi"])/96;
    int available=Convert.ToInt32(free["width"]);
    int height=Convert.ToInt32(free["height"]);
    int width=(int)Math.Ceiling(380*scale);
    int outerWidth=checked(width+frameExtraWidth);
    if(width<=0||height<=0||height>100000||available<outerWidth)throw new Exception("Not enough free taskbar space");
    int x=checked(Convert.ToInt32(free["x"])+available-outerWidth);
    int y=checked(Convert.ToInt32(free["y"])-clientTopOffset);
    return Rect(x,y,width,height);
  }
  static RECT ClientRectOnScreen(IntPtr h) {
    RECT client;
    if(!GetClientRect(h,out client))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
    int width=checked(client.right-client.left),height=checked(client.bottom-client.top);
    if(width<=0||height<=0||width>100000||height>100000)throw new Exception("Player client geometry is invalid");
    POINT origin=new POINT{x=client.left,y=client.top};
    if(!ClientToScreen(h,ref origin))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
    return new RECT{left=origin.x,top=origin.y,right=checked(origin.x+width),bottom=checked(origin.y+height)};
  }
  static void Place(Dictionary<string,object> rect,Dictionary<string,object> bar) {
    POINT p=new POINT{x=Convert.ToInt32(rect["x"]),y=Convert.ToInt32(rect["y"])};
    if(!ScreenToClient(parent,ref p))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
    int clientWidth=Convert.ToInt32(rect["width"]),clientHeight=Convert.ToInt32(rect["height"]);
    if(!SetWindowPos(window,IntPtr.Zero,p.x,p.y,checked(clientWidth+frameExtraWidth),checked(clientHeight+frameExtraHeight),0x10|0x20))
      throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
    RECT client=ClientRectOnScreen(window);
    var free=(Dictionary<string,object>)bar["free"];
    long left=Convert.ToInt32(free["x"]),top=Convert.ToInt32(free["y"]);
    long right=left+Convert.ToInt32(free["width"]),bottom=top+Convert.ToInt32(free["height"]);
    if(client.right-client.left!=clientWidth||client.bottom-client.top!=clientHeight||
      client.left<left||client.top<top||client.right>right||client.bottom>bottom)
      throw new Exception("Player client geometry does not fit the taskbar free area");
  }
  static void Restore() {
    if(attachedTo==IntPtr.Zero)return;
    if(OwnWindow()){
      SetParentChecked(oldParent);
      SetLongChecked(-16,oldStyle);SetLongChecked(-20,oldExStyle);
      if(!SetWindowPos(window,IntPtr.Zero,oldRect.left,oldRect.top,oldRect.right-oldRect.left,oldRect.bottom-oldRect.top,0x4|0x10|0x20|0x200))
        throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
      if(GetParent(window)!=oldParent)throw new Exception("Windows did not restore the player parent");
    }
    parent=IntPtr.Zero;attachedTo=IntPtr.Zero;
  }
  static object Run(Dictionary<string,object> req) {
    string op=(string)req["op"];
    if(op=="probe")return ProbeResult();
    if(!OwnWindow())throw new System.ComponentModel.Win32Exception(1400,"Lyrics window is no longer owned by the player");
    if(op=="show"){
      const uint noActivate=0x0010,noMove=0x0002,noSize=0x0001,noZOrder=0x0004,show=0x0040;
      IntPtr attachedHost=attachedTo;
      if(attachedHost!=IntPtr.Zero&&EnsureTaskbarChild(attachedHost))RepositionAfterFrameChange(attachedHost);
      if(!SetWindowPos(window,IntPtr.Zero,0,0,0,0,noActivate|noMove|noSize|noZOrder|show))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
      ShowWindow(window,4); // SW_SHOWNOACTIVATE
      if(attachedHost!=IntPtr.Zero&&EnsureTaskbarChild(attachedHost))RepositionAfterFrameChange(attachedHost);
      return new {visible=IsWindowVisible(window)};
    }
    if(op=="detach"){Restore();return new {ok=true};}
    if(op=="attach"){
      string handle=Convert.ToString(req["bar"]);
      var bar=FindBar(handle);
      IntPtr host=new IntPtr(long.Parse(handle));
      if(GetAwarenessFromDpiAwarenessContext(GetWindowDpiAwarenessContext(window))!=GetAwarenessFromDpiAwarenessContext(GetWindowDpiAwarenessContext(host)))
        throw new Exception("Taskbar and player DPI modes differ");
      Dictionary<string,object> placement;
      if(parent==IntPtr.Zero){
        if(!GetWindowRect(window,out oldRect))throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());oldParent=GetParent(window);
        // Preserve the floating frame before changing any native style bits.
        oldStyle=GetWindowLongPtr(window,-16);oldExStyle=GetWindowLongPtr(window,-20);
        parent=host;attachedTo=host;
        try {
          EnsureTaskbarChild(host);
          // The borderless child has different frame extents from the saved
          // floating window. Measure first, then fit its client to the free band.
          placement=Placement(bar);
          Place(placement,bar);
          if(EnsureTaskbarChild(host))RepositionAfterFrameChange(host);
        } catch {Restore();throw;}
      }else if(parent==host){
        EnsureTaskbarChild(host);
        placement=Placement(bar);Place(placement,bar);
        if(EnsureTaskbarChild(host))RepositionAfterFrameChange(host);
      }
      else throw new Exception("Detach before changing taskbars");
      return new {ok=true,rect=placement,parent=parent.ToInt64().ToString()};
    }
    throw new Exception("Unknown taskbar operation");
  }
  [STAThread] public static void Main(string[] args) {
    Console.InputEncoding=Encoding.UTF8;Console.OutputEncoding=new UTF8Encoding(false);
    SetThreadDpiAwarenessContext(new IntPtr(-4));
    if(args.Length!=2)return;
    owner=uint.Parse(args[0]);window=new IntPtr(long.Parse(args[1]));
    try{
      string line;
      while((line=Console.ReadLine())!=null){
        object id=null;
        try{
          if(line.Length>4096)throw new Exception("Request too large");
          var req=Json.Deserialize<Dictionary<string,object>>(line);id=req["id"];
          object result=Run(req);
          Console.WriteLine(Json.Serialize(new {id=id,result=result}));
        }catch(Exception e){Console.WriteLine(Json.Serialize(new {id=id,error=e.Message,code=e is System.ComponentModel.Win32Exception ? (int?)((System.ComponentModel.Win32Exception)e).NativeErrorCode : null}));}
      }
    }finally{try{Restore();}catch(Exception e){Console.Error.WriteLine(e.Message);}}
  }
}
