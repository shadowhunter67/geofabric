import os,subprocess,sys,json
cred=json.load(open(os.path.join(os.environ["LOCALAPPDATA"],"geofabric-runtime","postgres","credentials.json")))
psql=os.path.join(os.environ["LOCALAPPDATA"],"geofabric-runtime","postgres","pg16","pgsql","bin","psql.exe")
def q(sql,user="gf_admin"):
    r=subprocess.run([psql,"-h","127.0.0.1","-p","5433","-U",user,"-d","geofabric","-At","-F","|","-c",sql],capture_output=True,env=dict(os.environ,PGPASSWORD=cred[user],PGCLIENTENCODING="UTF8"))
    if r.returncode: sys.exit(r.stderr.decode("utf-8","replace")[-800:])
    return r.stdout.decode("utf-8").replace("\r\n","\n")
if __name__=="__main__": print(q(sys.argv[1]),end="")
