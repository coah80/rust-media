use blitz_dom::{
    BaseDocument, DocGuard, DocGuardMut, Document, DocumentConfig, EventDriver, EventHandler,
};
use blitz_html::HtmlDocument;
use blitz_traits::{
    events::{DomEvent, DomEventData, EventState, UiEvent},
    node_id::NodeId,
};
use boa_engine::{
    Context, JsArgs, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Source, js_string,
};
use std::{cell::RefCell, rc::Rc};

type Dom = Rc<RefCell<BaseDocument>>;

pub struct Page {
    dom: Dom,
    context: Context,
}

impl Page {
    pub fn new(html: &str) -> Result<Self, Box<dyn std::error::Error>> {
        if html.len() > 1024 * 1024 {
            return Err("HTML exceeds the prototype's 1 MiB limit".into());
        }
        let doc = HtmlDocument::from_html(html, DocumentConfig::default()).into_inner();
        if doc
            .query_selector("script[src],iframe,video,audio")
            .ok()
            .flatten()
            .is_some()
        {
            return Err(
                "External scripts, iframes and media elements are not implemented yet".into(),
            );
        }
        let scripts: Vec<_> = doc
            .query_selector_all("script")
            .map_err(|_| "Invalid selector")?
            .into_iter()
            .filter_map(|id| doc.get_node(id).map(|node| node.text_content()))
            .collect();
        let dom = Rc::new(RefCell::new(doc));
        let mut context = Context::default();
        context
            .runtime_limits_mut()
            .set_loop_iteration_limit(100_000);
        context.runtime_limits_mut().set_recursion_limit(128);
        context.insert_data(dom.clone());
        context.register_global_builtin_callable(
            js_string!("__query"),
            1,
            NativeFunction::from_fn_ptr(query),
        )?;
        context.register_global_builtin_callable(
            js_string!("__text"),
            1,
            NativeFunction::from_fn_ptr(text),
        )?;
        context.register_global_builtin_callable(
            js_string!("__setText"),
            2,
            NativeFunction::from_fn_ptr(set_text),
        )?;
        context.eval(Source::from_bytes(include_str!("dom.js")))?;
        for script in scripts {
            context.eval(Source::from_bytes(&script))?;
        }
        context.run_jobs()?;
        Ok(Self { dom, context })
    }

    fn click(&mut self, target: NodeId) {
        let source = format!("__dispatchClick(\"{}\")", target.as_u64());
        if let Err(error) = self
            .context
            .eval(Source::from_bytes(&source))
            .and_then(|_| self.context.run_jobs())
        {
            eprintln!("Page script failed: {error}");
        }
    }
}

impl Document for Page {
    fn inner(&self) -> DocGuard<'_> {
        DocGuard::RefCell(self.dom.borrow())
    }
    fn inner_mut(&mut self) -> DocGuardMut<'_> {
        DocGuardMut::RefCell(self.dom.borrow_mut())
    }

    fn handle_ui_event(&mut self, event: UiEvent) {
        let clicks = Rc::new(RefCell::new(Vec::new()));
        EventDriver::new(&mut self.dom, Clicks(clicks.clone())).handle_ui_event(event);
        for target in clicks.borrow().iter().copied() {
            self.click(target);
        }
    }
}

struct Clicks(Rc<RefCell<Vec<NodeId>>>);
impl EventHandler for Clicks {
    fn handle_event(
        &mut self,
        _chain: &[NodeId],
        event: &mut DomEvent,
        _: &mut dyn Document,
        _: &mut EventState,
    ) {
        if matches!(event.data, DomEventData::Click(_)) {
            self.0.borrow_mut().push(event.target);
        }
    }
}

fn query(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let selector = args
        .get_or_undefined(0)
        .to_string(context)?
        .to_std_string_escaped();
    let dom = context.get_data::<Dom>().unwrap().borrow();
    let id = dom
        .query_selector(&selector)
        .map_err(|_| JsNativeError::syntax().with_message("Invalid selector"))?;
    Ok(id
        .map(|id| JsValue::from(JsString::from(id.as_u64().to_string())))
        .unwrap_or_else(JsValue::null))
}

fn text(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let raw = args
        .get_or_undefined(0)
        .to_string(context)?
        .to_std_string_escaped()
        .parse::<u64>()
        .map_err(|_| JsNativeError::typ().with_message("Invalid node id"))?;
    let id = NodeId::from_u64(raw);
    let dom = context.get_data::<Dom>().unwrap().borrow();
    let node = dom
        .get_node(id)
        .ok_or_else(|| JsNativeError::typ().with_message("Missing DOM node"))?;
    Ok(JsString::from(node.text_content()).into())
}

fn set_text(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let raw = args
        .get_or_undefined(0)
        .to_string(context)?
        .to_std_string_escaped()
        .parse::<u64>()
        .map_err(|_| JsNativeError::typ().with_message("Invalid node id"))?;
    let id = NodeId::from_u64(raw);
    let value = args
        .get_or_undefined(1)
        .to_string(context)?
        .to_std_string_escaped();
    if value.len() > 64 * 1024 {
        return Err(JsNativeError::range()
            .with_message("Text exceeds the prototype limit")
            .into());
    }
    let mut doc = context.get_data::<Dom>().unwrap().borrow_mut();
    if doc.get_node(id).is_none() {
        return Err(JsNativeError::typ().with_message("Missing DOM node").into());
    }
    let mut edit = doc.mutate();
    edit.remove_and_drop_all_children(id);
    let child = edit.create_text_node(&value);
    edit.append_children(id, &[child]);
    Ok(JsValue::undefined())
}
