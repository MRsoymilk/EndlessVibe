use crate::config::Config;
use url::Url;

pub fn valid_redirect(config:&Config,value:&str)->bool{
    let Ok(url)=Url::parse(value)else{return false;};
    if !url.username().is_empty()||url.password().is_some()||url.fragment().is_some(){return false;}
    if config.security.extra_redirect_uris.iter().any(|u|u==value){
        return url.scheme()=="https"||(config.security.allow_http_loopback&&url.scheme()=="http"&&matches!(url.host_str(),Some("localhost"|"127.0.0.1"|"[::1]"|"::1")));
    }
    if url.scheme()!="https"||url.host_str()!=Some("chatgpt.com")||url.port_or_known_default()!=Some(443)||url.query().is_some(){return false;}
    if url.path()=="/connector_platform_oauth_redirect"{return true;}
    url.path().strip_prefix("/connector/oauth/").is_some_and(|id|!id.is_empty()&&id.len()<=160&&id.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_'||b==b'-'))
}

pub(super) fn pkce_verifier(value:&str)->bool{(43..=128).contains(&value.len())&&value.bytes().all(|b|b.is_ascii_alphanumeric()||b"-._~".contains(&b))}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn callbacks_are_not_open_redirects(){let c=Config::default();assert!(valid_redirect(&c,"https://chatgpt.com/connector_platform_oauth_redirect"));assert!(valid_redirect(&c,"https://chatgpt.com/connector/oauth/plugin_123"));for u in ["https://evil.test/connector_platform_oauth_redirect","https://chatgpt.com.evil.test/connector/oauth/x","https://chatgpt.com@evil.test/x","https://chatgpt.com/connector/oauth/../../evil","http://chatgpt.com/connector_platform_oauth_redirect","https://chatgpt.com/connector/oauth/x?next=https://evil.test"]{assert!(!valid_redirect(&c,u),"{u}");}}
    #[test]fn pkce_requires_strong_verifier(){assert!(pkce_verifier(&"x".repeat(43)));assert!(!pkce_verifier("short"));assert!(!pkce_verifier(&" ".repeat(43)));}
}
